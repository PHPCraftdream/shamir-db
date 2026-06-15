//! Admin handlers: SetRetention, PurgeHistory, ChangesSince.

use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, apply_table_retention, resolve_table_mvcc};

impl ShamirAdminExecutor {
    // T3: change a live table's history-retention policy on the fly.
    pub(super) async fn handle_set_retention(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::SetRetention(op) = batch_op else {
            unreachable!("handle_set_retention called with non-SetRetention op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.set_retention.clone(),
                ),
                Action::Manage,
            )
            .await
            .map_err(err_access)?;
        op.retention.validate().map_err(err)?;
        let policy = crate::engine::repo::to_mvcc_retention(&op.retention);
        apply_table_retention(
            &self.shamir,
            &self.db_name,
            &op.repo,
            &op.set_retention,
            policy,
        )
        .await?;
        Ok(admin_result(json!({
            "set_retention": op.set_retention,
            "repo": op.repo,
            "ok": true
        })))
    }

    // T4-purge: imperative one-shot history purge by a time
    // predicate. Mirrors SetRetention's table-scoped auth +
    // per_table_mvcc lookup, then resolves the cutoff against
    // the MvccStore's OWN clock (so OlderThanAge is
    // deterministic under set_test_now) and calls
    // purge_below_ts. Sacred MVCC invariants (snapshot floor,
    // anchor, unknown-ts-kept) are enforced inside the store.
    pub(super) async fn handle_purge_history(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::PurgeHistory(op) = batch_op else {
            unreachable!("handle_purge_history called with non-PurgeHistory op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.purge_history.clone(),
                ),
                Action::Manage,
            )
            .await
            .map_err(err_access)?;
        let mvcc =
            resolve_table_mvcc(&self.shamir, &self.db_name, &op.repo, &op.purge_history).await?;

        // D2 P1d-2b: drain the repo's inflight WAL tail into `history` BEFORE
        // purging. Post-cutover, freshly-committed versions live in the
        // in-memory overlay until the background drainer lands them in history;
        // a purge that scanned history first would miss (and so fail to
        // reclaim, or mis-resolve the ts cutoff against) the undrained tail.
        // `drain_all` is the authoritative warm-drain (= generalized recovery)
        // and is idempotent / cheap when already caught up.
        if let Some(repo) = self
            .shamir
            .get_db(&self.db_name)
            .and_then(|db| db.get_repo(&op.repo))
        {
            if let Err(e) = repo.drainer().drain_all(&repo).await {
                log::warn!(
                    "handle_purge_history: drain_all {}/{}: {e}",
                    op.repo,
                    op.purge_history
                );
            }
        }

        // Resolve the cutoff from the scope. OlderThan is an
        // absolute epoch-millis; OlderThanAge is subtracted
        // from the store's clock so tests freeze the clock via
        // set_test_now and get a deterministic cutoff.
        let cutoff = match op.scope {
            crate::query::admin::PurgeScope::OlderThan { timestamp } => timestamp,
            crate::query::admin::PurgeScope::OlderThanAge { age_secs } => mvcc
                .clock_millis()
                .saturating_sub(age_secs.saturating_mul(1000)),
        };
        let purged = mvcc
            .purge_below_ts(cutoff)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "purge_history": op.purge_history,
            "repo": op.repo,
            "purged": purged
        })))
    }

    // T4-changes-since: one-shot "changes since version V" journal
    // read. A read-style admin op: authorizes Action::Read on the
    // repo (Store) resource, then range-reads the durable changelog
    // journal for events with commit_version strictly greater than
    // the client's cursor, and surfaces the CF-1 `gap_at` re-sync
    // marker. Read-only — no live push, no journal-write change.
    pub(super) async fn handle_changes_since(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::ChangesSince(op) = batch_op else {
            unreachable!("handle_changes_since called with non-ChangesSince op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::store(self.db_name.clone(), op.repo.clone()),
                Action::Read,
            )
            .await
            .map_err(err_access)?;
        // cursor + 1: read_from returns events with commit_version >=
        // from_version, and the contract is "strictly after the cursor".
        let from_version = op
            .changes_since
            .checked_add(1)
            .ok_or_else(|| err("changes_since cursor overflow".to_string()))?;
        let limit = op.limit.unwrap_or(1000) as usize;
        let jr = match self
            .shamir
            .read_changelog_from_journal(&self.db_name, &op.repo, from_version, limit)
            .await
        {
            Some(jr) => jr,
            None => {
                return Err(err(format!(
                    "Repository '{}.{}' not found",
                    self.db_name, op.repo
                )))
            }
        };
        // Serialize the events via serde_json (ChangelogEvent is
        // Serialize). gap_at is surfaced verbatim (null when no gap).
        let events_json: Vec<serde_json::Value> = jr
            .events
            .iter()
            .map(|e| serde_json::to_value(e).map_err(|e| err(e.to_string())))
            .collect::<Result<_, _>>()?;
        Ok(admin_result(json!({
            "changes_since": op.changes_since,
            "events": events_json,
            "gap_at": jr.gap_at,
        })))
    }
}
