use crate::shamir_db::execute::quarantine_helpers::admin_replication::{
    quarantine_row_entries, row_is_quarantined,
};
use crate::types::value::QueryValue;
use crate::{DbResult, ShamirDb};
use shamir_types::mpack;

impl ShamirDb {
    /// Does any subscription in `state = "resync_required"` cover
    /// `(db_name, repo_name)` with a `Pull` stream?
    ///
    /// Cold-ish path: reads the whole `system/subscriptions` table and
    /// resolves each resync_required row's profile streams
    /// (`replication_profiles` table). Called by the replication serve path
    /// (leader hello/pull in `shamir-server`) to refuse serving a repo whose
    /// downstream destination is quarantined for reseed (S5c-X 2e).
    /// scope.repo `None` matches every repo in the scope's db.
    pub async fn has_resync_required_subscription_covering(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<bool> {
        let entries = self.resync_quarantine_entries().await?;
        Ok(entries
            .iter()
            // db == "*" mirrors the read-side ("*", "*") global sentinel.
            .any(|((db, repo), _)| {
                (db == "*" || db == db_name) && (repo == "*" || repo == repo_name)
            }))
    }
    /// Single source of truth for the quarantine set: walk the
    /// `system/subscriptions` table rows that are quarantined
    /// ([`row_is_quarantined`]) and resolve each row's profile `Pull` streams
    /// into cache keys
    /// `(db, repo)` (or `(db, "*")` when `scope.repo` is `None`), valued
    /// with the row's `status_message` (or a constant reason when absent).
    /// Consumed by both [`Self::has_resync_required_subscription_covering`]
    /// and [`Self::refresh_resync_quarantine_cache`].
    ///
    /// Fail-closed fallback: a row whose profile is missing/unresolvable
    /// contributes the global ("*", "*") sentinel instead of an error (see
    /// [`quarantine_row_entries`]); infra failures of the `subscriptions`
    /// table read itself still propagate via `?`.
    pub(crate) async fn resync_quarantine_entries(
        &self,
    ) -> DbResult<Vec<((String, String), String)>> {
        let mut entries = Vec::new();
        let table = self.system_store().subscriptions_table().await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let result = table
            .read(&crate::query::read::ReadQuery::new("subscriptions"), &ctx)
            .await?;
        for record in result.records {
            let row = record.as_value().into_owned();
            if !row_is_quarantined(&row) {
                continue;
            }
            let profile = row.get("profile").and_then(|value| value.as_str());
            let reason = row
                .get("status_message")
                .and_then(|value| value.as_str())
                .unwrap_or("subscription requires resync");
            let (mut row_entries, failure) = quarantine_row_entries(self, profile, reason).await;
            if let Some(failure) = &failure {
                log::warn!(
                    "resync_quarantine_entries: quarantined row's profile \
                     unresolvable ({failure}); global read quarantine installed"
                );
            }
            entries.append(&mut row_entries);
        }
        Ok(entries)
    }

    /// Quarantine reason for an ordinary READ of `(db_name, repo_name)`, or
    /// `None` when the repo is readable. Healthy path (empty cache): one
    /// `Relaxed` `is_empty` load and no allocation / map lock — this is what
    /// the wire-layer cursor gate in `shamir-server` rides on.
    pub(crate) fn quarantined_read_reason(&self, db_name: &str, repo_name: &str) -> Option<String> {
        // Healthy-path fast path: one Relaxed load, no allocation, no map
        // lock when the cache is empty (no subscription is quarantined).
        if self.resync_quarantine.is_empty() {
            return None;
        }
        self.resync_quarantine.reason_for(db_name, repo_name)
    }

    /// Public seam for wire-layer read gates (e.g. cursor-based journal reads
    /// in the server crate): the read-quarantine (`resync_required`) reason
    /// for `(db_name, repo_name)`, or `None` when the repo is readable.
    /// Mirrors the internal gate of
    /// [`read_changelog_bounded`](crate::ShamirDb::read_changelog_bounded).
    pub fn read_quarantine_reason(&self, db_name: &str, repo_name: &str) -> Option<String> {
        self.quarantined_read_reason(db_name, repo_name)
    }

    /// Persist `state = "resync_required"` on subscription `name`'s row in
    /// `system/subscriptions`.
    ///
    /// Called by the follower-loop supervisor (`shamir-server`) when
    /// [`run_follower_loop`](../../../shamir-server/src/replication/follower_loop.rs)
    /// (out of this crate) returns a terminal journal-gap error: the
    /// follower is permanently missing `[from_version, gap_at)` and must
    /// stop, and this makes that stop visible via the existing
    /// `ReplicationStatus`/`ListSubscriptions` admin surface (both already
    /// echo `state` verbatim). Because `reconcile()` only (re)starts
    /// subscriptions whose row has `state == "active"`, a row left at
    /// `"resync_required"` naturally stays stopped across reconcile ticks,
    /// exactly like `"paused"` does today. Recovery requires a replacement seed
    /// install (which binds a NEW `seed_receipt` on the row, differing from
    /// the `resync_from_receipt` baseline captured here) followed by the
    /// `Resume` admin action — a plain `Resume` of a still-quarantined row
    /// is refused with a typed error. The quarantine is additionally marked
    /// with a durable `quarantined` flag on the row, which — unlike `state`
    /// — survives Pause/SetProfile: any admin action on a quarantined row
    /// except a post-reseed `Resume` (or a `DROP`) is refused.
    ///
    /// `name` not found → treated as a no-op (logged, not errored): the
    /// subscription may have been dropped concurrently by an admin command
    /// racing with the gap detection.
    pub async fn mark_subscription_resync_required(
        &self,
        name: &str,
        gap_at: u64,
        from_version: u64,
    ) -> DbResult<()> {
        self.mark_subscription_terminal(
            name,
            "resync_required",
            &format!("journal gap at {gap_at}, requested from {from_version}"),
        )
        .await
    }

    /// Persist an operational stop reason exposed verbatim by status/list.
    /// A `"resync_required"` state additionally stamps the durable
    /// `quarantined` marker plus the `resync_from_receipt` baseline (see
    /// [`Self::mark_subscription_resync_required`]); other states never set
    /// the marker.
    ///
    /// Marking NEVER fails because the covered set cannot be computed:
    /// a missing/unresolvable profile falls back to the global ("*", "*")
    /// sentinel (see [`quarantine_row_entries`]), logged and appended to
    /// `status_message`; the persisted write always happens.
    pub async fn mark_subscription_terminal(
        &self,
        name: &str,
        state: &str,
        message: &str,
    ) -> DbResult<()> {
        let _control = self.replication_control_lock.lock().await;
        let mut next_entries = self.resync_quarantine_entries().await?;
        let table = self.system_store().subscriptions_table().await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let lookup = crate::query::read::ReadQuery::new("subscriptions").filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        );
        let existing = table.read(&lookup, &ctx).await?;
        let Some(row) = existing.records.into_iter().next() else {
            log::warn!(
                "mark_subscription_resync_required: subscription '{name}' not found \
                 (state={state}); treating as a no-op \
                 — it may have been dropped concurrently"
            );
            return Ok(());
        };

        // Row-level profile resolution failure — surfaced below via
        // `status_message`, never as a marking error (fail-closed fallback).
        let mut resolution_failure = None;
        if state == "resync_required" {
            let owned_row = row.as_value().into_owned();
            let profile = owned_row.get("profile").and_then(|value| value.as_str());
            let (mut row_entries, failure) = quarantine_row_entries(self, profile, message).await;
            next_entries.append(&mut row_entries);
            if let Some(failure) = &failure {
                log::warn!(
                    "mark_subscription_terminal: {failure}; \
                     global read quarantine installed"
                );
                resolution_failure = Some(failure.clone());
            }
            self.resync_quarantine.publish(next_entries);
            Box::pin(self.revoke_quarantined_receivers()).await; // Round-14 boxed
        }
        let mut record = row.as_value().into_owned();
        // S5c-X PR8b: capture the row's CURRENT `seed_receipt` as the durable
        // baseline the quarantine began from (absent ⇒ `Null`). `Resume`'s
        // gate requires the bound receipt to differ from this value, so a
        // plain `Resume` can never lift the quarantine without a replacement
        // seed installed in between.
        let resync_from_receipt = record
            .get("seed_receipt")
            .cloned()
            .unwrap_or(QueryValue::Null);
        if let QueryValue::Map(ref mut m) = record {
            m.insert("state".to_string(), QueryValue::Str(state.to_string()));
            m.insert("resync_from_receipt".into(), resync_from_receipt);
            if state == "resync_required" {
                // Durable quarantine marker: unlike `state`, it is not
                // cleared by Pause/SetProfile — only a successful post-reseed
                // `Resume` (or DROP) removes it.
                m.insert("quarantined".into(), QueryValue::Bool(true));
            }
        }
        let mut status_message = message.to_string();
        if let Some(failure) = &resolution_failure {
            // Surface the resolution failure via status_message (echoed
            // verbatim by status/list), not as a marking error.
            status_message = format!("{message}; {failure}");
        }
        if let QueryValue::Map(ref mut fields) = record {
            fields.insert(
                "last_terminal_reason".into(),
                QueryValue::Str(state.to_string()),
            );
            fields.insert("status_message".into(), QueryValue::Str(status_message));
        }
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("subscriptions"),
            key: mpack!({"name": @(QueryValue::Str(name.to_string()))}),
            value: record,
        };
        self.system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await?;
        self.system_store()
            .persist_table_interner(&table, None)
            .await?;
        self.flush_seed_catalogue().await?;
        Box::pin(self.refresh_resync_quarantine_cache()).await?; // Round-14 boxed
        Ok(())
    }
}
