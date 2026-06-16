//! Admin handlers: InternerDump, InternerTouch (Stage 5d).
//!
//! Both ops target the per-repo interner that lives on `RepoInstance`.
//! They are transport-only: the server resolves ids / names; building a
//! client-side auto-cache is deferred to Stage 5.

use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

impl ShamirAdminExecutor {
    /// Dump the repo interner dictionary (id → name).
    ///
    /// Read-style admin op: authorizes `Action::Read` on the repo Store
    /// resource. Without `since` the full dictionary is returned; with
    /// `since` only entries with id > `since` are (delta refresh).
    ///
    /// The reported `epoch` is the highest gap-free id present in the
    /// returned snapshot — the `entries_after` high-water mark, or for a
    /// full dump the max id among `all_entries()`. This is deliberately
    /// NOT `interner.len()`: under concurrent `touch_ind` the forward
    /// map's `len()` can outrun the reverse vec by a window, so using
    /// `len()` would advertise an id whose name was not actually returned.
    pub(super) async fn handle_interner_dump(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::InternerDump(op) = batch_op else {
            unreachable!("handle_interner_dump called with non-InternerDump op");
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
                &ResourcePath::store(self.db_name.clone(), op.interner_dump.clone()),
                Action::Read,
            )
            .await
            .map_err(err_access)?;

        let repo = self
            .shamir
            .get_db(&self.db_name)
            .and_then(|db| db.get_repo(&op.interner_dump))
            .ok_or_else(|| {
                err(format!(
                    "Repository '{}.{}' not found",
                    self.db_name, op.interner_dump
                ))
            })?;
        let mgr = repo.repo_interner().await.map_err(|e| err(e.to_string()))?;
        let interner = mgr.get().await.map_err(|e| err(e.to_string()))?;

        // Capture entries + the high-water epoch. When `since` is set we
        // ask `entries_after` for the delta + its gap-free high-water
        // mark; for a full dump we take `all_entries` and derive the
        // epoch as the max id present (NOT len() — see the doc comment).
        let (raw, epoch) = if let Some(since) = op.since {
            let (entries, high) = interner.entries_after(since as usize);
            (entries, high as u64)
        } else {
            let entries = interner.all_entries();
            let high = entries.iter().map(|(k, _)| k.id()).max().unwrap_or(0);
            (entries, high)
        };

        let entries: Vec<(u64, String)> = raw
            .into_iter()
            .map(|(k, u)| (k.id(), u.as_str().to_owned()))
            .collect();

        Ok(admin_result(json!({
            "interner_dump": op.interner_dump,
            "epoch": epoch,
            "entries": entries,
        })))
    }

    /// Register field NAMES, returning the (name → id) mapping.
    ///
    /// Write-style admin op: authorizes `Action::Write` on the repo Store
    /// resource. Anyone who can write records already interns via the
    /// write path (`write_exec`), so touch is the explicit equivalent.
    /// Newly-minted ids are persisted (`mgr.persist()`) so they survive a
    /// crash. §9.4: a key is ALWAYS a name; `"42"` interns to whatever id
    /// the interner assigns, never raw id 42.
    pub(super) async fn handle_interner_touch(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::InternerTouch(op) = batch_op else {
            unreachable!("handle_interner_touch called with non-InternerTouch op");
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
                &ResourcePath::store(self.db_name.clone(), op.interner_touch.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        let repo = self
            .shamir
            .get_db(&self.db_name)
            .and_then(|db| db.get_repo(&op.interner_touch))
            .ok_or_else(|| {
                err(format!(
                    "Repository '{}.{}' not found",
                    self.db_name, op.interner_touch
                ))
            })?;
        let mgr = repo.repo_interner().await.map_err(|e| err(e.to_string()))?;
        let interner = mgr.get().await.map_err(|e| err(e.to_string()))?;

        // Intern each name idempotently; touch_ind returns New | Exists,
        // both carry the assigned InternerKey. A name already present
        // returns its existing id (idempotent re-touch).
        let mappings: Vec<(String, u64)> = op
            .names
            .iter()
            .map(|name| {
                let key = interner
                    .touch_ind(name)
                    .map_err(|e| err(format!("interner touch '{name}': {e}")))?
                    .into_key();
                Ok((name.clone(), key.id()))
            })
            .collect::<Result<_, BatchError>>()?;

        // Durability: newly-minted ids must survive a crash. persist()
        // writes the delta since the last chunk as an append-only chunk.
        mgr.persist().await.map_err(|e| err(e.to_string()))?;

        let epoch = interner
            .all_entries()
            .iter()
            .map(|(k, _)| k.id())
            .max()
            .unwrap_or(0);

        Ok(admin_result(json!({
            "interner_touch": op.interner_touch,
            "epoch": epoch,
            "mappings": mappings,
        })))
    }
}
