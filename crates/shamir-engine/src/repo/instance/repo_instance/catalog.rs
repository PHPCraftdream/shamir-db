use crate::table::table_manager::table_token_for;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Repo;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::OnceCell;

use super::RepoInstance;

mod repo_membership_impl;
pub(crate) mod visibility_impl;

impl RepoInstance {
    // ============================================================================
    // Index Management API (proxy to TableManager)
    // ============================================================================

    /// cancel-safe: NO — multi-step state mutation. Looks up the table
    /// (one await) then calls `table.create_index` which itself performs
    /// catalogue updates and persistence. Partial cancellation can leave
    /// the index in an inconsistent state; recovery is at the caller's
    /// discretion.
    ///
    /// Create a regular index on a table.
    pub async fn create_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_index(index_name, paths).await
    }

    /// cancel-safe: NO — same shape as `create_index`. Catalogue and
    /// persistence updates inside `table.create_unique_index` are not
    /// atomic across cancellation.
    ///
    /// Create a unique index on a table.
    pub async fn create_unique_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_unique_index(index_name, paths).await
    }

    /// cancel-safe: NO — multi-step state mutation: lookup table then
    /// delete catalogue entries and persisted index data. Partial
    /// cancellation may leave orphaned index state.
    ///
    /// Drop a regular index from a table.
    pub async fn drop_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_index(index_name, None).await
    }

    /// cancel-safe: NO — same shape as `drop_index`. Partial cancellation
    /// may leave orphaned unique-index state.
    ///
    /// Drop a unique index from a table.
    pub async fn drop_unique_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_unique_index(index_name, None).await
    }

    /// cancel-safe: yes — read-only: looks up the table and queries
    /// existence. No state mutation; cancellation drops the query.
    ///
    /// Check if a regular index exists on a table.
    pub async fn index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.index_exists(index_name).await)
    }

    /// cancel-safe: yes — read-only existence query. No state mutation.
    ///
    /// Check if a unique index exists on a table.
    pub async fn unique_index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.unique_index_exists(index_name).await)
    }

    /// cancel-safe: yes — read-only lookup. Cancellation drops the query
    /// future with no state mutation.
    ///
    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        table_name: &str,
        index_name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<shamir_types::types::record_id::RecordId>> {
        let table = self.get_table(table_name).await?;
        table.lookup_by_index(index_name, values).await
    }

    /// cancel-safe: yes — one `scc::HashMap` read plus a single
    /// `get_table` call. Cancellation leaves no state mutated (apart from
    /// the cancel-safe `get_table` OnceCell init).
    ///
    /// Look up the table whose token matches `token`. Used by V2 WAL
    /// recovery to resolve ops by `table_id_interned`, and by the commit
    /// pipeline (Phases 1, 2.6, 5b–5d) while holding the per-repo
    /// `commit_lock`.
    ///
    /// O(1) (III.1): resolves through the `token_names` reverse index that
    /// `add_table` maintains, instead of scanning every config and
    /// re-hashing its name. The old O(N) scan grew the serialized
    /// critical section linearly with schema size. `add_table` and
    /// `remove_table` keep `token_names` in lock-step with `configs`, so a
    /// resolved name always still has a config; `get_table` re-validates
    /// against `configs` inside its init closure regardless.
    pub async fn table_by_token(&self, token: u64) -> DbResult<Option<TableManager>> {
        // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
        // H1+H2 commit `621776bd`): `read_sync`, NOT `read_async`. The
        // `token_names` map is also touched SYNCHRONOUSLY by `register_token`'s
        // `insert_sync` + `read_sync` (DDL — table create/rename) and by
        // drop-table's `remove_if_sync` (DDL). `read_async`'s wait is
        // lock-HANDOFF: saa grants the shared bucket lock to the suspended
        // reader TASK, which then holds it while unpolled in tokio's run queue;
        // a DDL exclusive writer (create/rename/drop) racing sustained
        // commit-pipeline / V2-WAL-recovery `read_async` traffic can park every
        // worker behind that unpolled reader → whole-runtime deadlock.
        // `read_sync`'s bucket lock is held only by a RUNNING thread for a few
        // instructions (a `String` clone), bounding every wait. The fn stays
        // `async`; this call no longer suspends.
        let name = self.token_names.read_sync(&token, |_, name| name.clone());
        match name {
            Some(name) => {
                let tbl = self.get_table(&name).await?;
                Ok(Some(tbl))
            }
            None => Ok(None),
        }
    }

    /// Non-instantiating peek: like [`table_by_token`](Self::table_by_token),
    /// but returns `None` (rather than lazily creating and returning) for a
    /// table that is registered (`add_table` was called) but has never been
    /// touched via [`get_table`](Self::get_table) — the `OnceCell` is still
    /// dormant.
    ///
    /// #538 Part A: `pre_commit_prelock`'s Phase 2.5 uses this instead of
    /// `table_by_token` to check `needs_write_barrier()` for every table in
    /// `tx.write_set`, specifically so that check-only lookup does NOT force
    /// lazy table instantiation as a side effect. This matters because
    /// instantiating a `TableManager` registers it in `per_table_mvcc`,
    /// which flips `apply_data_batch`'s routing for that table's data writes
    /// from the direct-store fallback to MVCC-routed — a behavior change
    /// nothing in `pre_commit_prelock` should trigger merely by CHECKING a
    /// barrier flag. Correctness is preserved: `needs_write_barrier()` can
    /// only ever be `true` on an instance that was actually created — no
    /// code path can flip `index2_create_barrier` or register a base_index
    /// unique index without first holding a live `TableManager` (`get_table`
    /// must have run at least once for a `create_index_v2` /
    /// `create_unique_index` to even be callable) — so a dormant `OnceCell`
    /// can only ever mean "this table has no in-flight barrier", making
    /// `None` here exactly equivalent to "table needs no barrier" for this
    /// check's purposes.
    pub(crate) async fn table_by_token_if_live(&self, token: u64) -> Option<TableManager> {
        // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
        // H1+H2 commit `621776bd`): `read_sync`, NOT `read_async` (same
        // `token_names` map `table_by_token` above also now uses). The map is
        // written EXCLUSIVELY by DDL (`register_token`'s `insert_sync`;
        // drop-table's `remove_if_sync`), and `read_async`'s wait is
        // lock-HANDOFF — a DDL writer racing sustained commit/recovery reads can
        // park every worker behind an unpolled handed-off reader →
        // whole-runtime deadlock. `read_sync`'s bucket lock is held only by a
        // RUNNING thread for a few instructions (a `String` clone), bounding
        // every wait.
        //
        // NON-INSTANTIATING PROPERTY PRESERVED: this change is purely the
        // lock-acquisition mechanism. The lookup still reads the dormant
        // `tables` OnceCell map DIRECTLY (`self.tables.get(&name)`) and does
        // NOT call the lazily-instantiating `get_table` — so the documented
        // reason this fn exists (Phase 2.5's barrier check must NOT force table
        // instantiation as a side effect) is unaffected.
        let name = self.token_names.read_sync(&token, |_, name| name.clone())?;
        let cell = self
            .tables
            .get(&name)
            .map(|entry| Arc::clone(entry.value()))?;
        cell.get().cloned()
    }
}

impl RepoInstance {
    /// Test-only fault-injection seam: force-overwrite the cached
    /// `TableManager` for `table_name` with a caller-supplied instance
    /// (e.g. one built via [`TableManager::create`] on a fault-injecting
    /// `Store` double), so subsequent [`get_table`](Self::get_table) calls
    /// on THIS repo instance (and any clone sharing the same `tables` map)
    /// observe it. Used by `shamir-db`'s resource_meta fail-closed
    /// regression test (audit #540) to make a real catalogue read return
    /// `Err` without a large `BoxRepo`/`BoxRepoFactory` fault-injection
    /// rewrite.
    ///
    /// Gated by the `test-util` feature (NOT plain `#[cfg(test)]`) because
    /// the caller is a downstream crate's test (`shamir-db`) — a
    /// `#[cfg(test)]` item is private to the crate being compiled in test
    /// mode and would not exist when `shamir-engine` is pulled in as a
    /// normal (non-test) dependency. Never compiled into a production
    /// build: no default feature enables `test-util`.
    ///
    /// **Caveat for reuse**: this only overwrites the `tables` cache, NOT
    /// `configs`. Safe when `table_name` is already registered in `configs`
    /// (true for every existing caller — e.g. `SystemStore::init` registers
    /// `"databases"` before this is ever called). Calling this with a
    /// table name that is NOT already in `configs` would desync
    /// `has_table`/`list_table_names`/`token_names` from `tables` — if a
    /// future caller needs that, register the table via the normal config
    /// path first.
    #[cfg(feature = "test-util")]
    pub fn install_table_for_test(&self, table_name: &str, tbl: TableManager) {
        self.tables.insert(
            table_name.to_string(),
            Arc::new(OnceCell::new_with(Some(tbl))),
        );
    }

    pub fn list_table_names(&self) -> Vec<String> {
        self.configs.iter().map(|e| e.key().clone()).collect()
    }

    /// #984 — count degraded (non-`Ready`) indexes across all ALREADY-OPEN
    /// tables in this repo. Pure in-memory — zero store reads. Does NOT
    /// force-open closed tables (that would turn a cheap `/metrics` gauge
    /// into an expensive scan and change server behaviour as a side-effect
    /// of scraping). See [`TableManager::degraded_index_count`] for the
    /// per-table cost model.
    pub async fn degraded_index_count(&self) -> u64 {
        // Collect Arc<OnceCell> clones first to avoid holding DashMap read
        // guards across the `.await` points inside the loop.
        let cells: Vec<Arc<OnceCell<TableManager>>> =
            self.tables.iter().map(|e| Arc::clone(e.value())).collect();
        let mut count = 0u64;
        for cell in &cells {
            if let Some(tm) = cell.get() {
                count += tm.degraded_index_count().await;
            }
        }
        count
    }

    pub fn has_table(&self, table_name: &str) -> bool {
        self.configs.contains_key(table_name)
    }

    pub fn table_count(&self) -> usize {
        self.configs.len()
    }

    /// Rename a table's live registration: copies the three physical
    /// data stores (`__data__`, `__info__`, `__history__`) from the old
    /// name to the new one via [`Repo::copy_store`], then swaps the
    /// in-memory config + reverse-index entry. Returns `false` if the
    /// source table was not configured.
    ///
    /// The OLD physical stores are intentionally orphaned (same
    /// disposition as `drop_table`, which orphans `__data__` because
    /// the catalogue is the source of truth). If the caller also needs
    /// the old stores gone, follow up with `store_delete` per namespace.
    pub async fn rename_table_stores(&self, from: &str, to: &str) -> DbResult<bool> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.rename_table_stores_admitted(from, to, &permit).await
    }

    /// Rename while holding this repository's write-admission permit.
    pub async fn rename_table_stores_admitted(
        &self,
        from: &str,
        to: &str,
        permit: &crate::repo::changelog_store::write_domain::RepoWritePermit,
    ) -> DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        // F-68 (#895) cluster D / task #124 diagnostic instrumentation:
        // timestamped entry/exit — NOT a behavior change, logging only.
        let rts_started = std::time::Instant::now();
        log::debug!("rename_table_stores: enter from={from} to={to}");
        // Snapshot the old config BEFORE mutating `configs` so a missing
        // table is reported cleanly and a concurrent `remove_table` can
        // only make us return `false` (no half-applied rename).
        let old_config = match self.configs.get(from) {
            Some(c) => c.clone(),
            None => return Ok(false),
        };

        // 0. Force-drain the source table's MVCC overlay into
        //    `__history__<from>` so the copy picks up EVERY committed row.
        //    `drain_to_history` snapshots the overlay up to the visibility
        //    watermark, writes each version through
        //    `write_committed_to_history`, advances the durable watermark,
        //    and reclaims the overlay — synchronously. Without this, rows
        //    that still live only in the in-memory overlay (not yet drained
        //    by the background drainer) would be missed by the store copy.
        //    Idempotent: if the overlay is already drained (all entries are
        //    in history), this is a no-op.
        let from_token = table_token_for(from);
        // Clone the Arc out: `get_sync`'s entry guard holds the bucket lock,
        // and the drainer's `iter_sync` over this map parks its thread on it,
        // so holding it across the awaits below wedges a current-thread
        // runtime (#589 class).
        let from_mvcc = self
            .per_table_mvcc
            .read_sync(&from_token, |_, mvcc| Arc::clone(mvcc));
        if let Some(mvcc) = from_mvcc {
            // F-68 (#895) cluster D / task #124: timestamped before/after
            // around `drain_to_history` — this synchronously walks the
            // overlay and writes each drained version to `__history__`, the
            // one potentially I/O-heavy step in this function.
            let drain_started = std::time::Instant::now();
            log::debug!("rename_table_stores: calling drain_to_history from={from}");
            mvcc.drain_to_history().await?;
            log::debug!(
                "rename_table_stores: drain_to_history returned after {:?}",
                drain_started.elapsed()
            );
        }

        // 1. Copy physical stores under the new name. `copy_store` is
        //    idempotent on an empty destination (a fresh store).
        self.repo
            .copy_store(&format!("__data__{}", from), &format!("__data__{}", to))
            .await?;
        self.repo
            .copy_store(&format!("__info__{}", from), &format!("__info__{}", to))
            .await?;
        self.repo
            .copy_store(
                &format!("__history__{}", from),
                &format!("__history__{}", to),
            )
            .await?;

        // 2. Drop the old live registration. `remove_table` also clears
        //    the in-memory `OnceCell<TableManager>`, the old
        //    `token_names` reverse-index entry, AND the old
        //    `per_table_mvcc` registration (A13) so a future CREATE under
        //    the `from` name re-registers cleanly.
        let _ = self.remove_table(from, permit);

        // 3. Register the new table config (preserving `enable_indexes`)
        //    and install its reverse-index entry. Reuses `add_table` so
        //    the new `OnceCell` lazily constructs a fresh `TableManager`
        //    pointing at the just-copied stores on first access.
        let new_config = TableConfig {
            name: to.to_string(),
            enable_indexes: old_config.enable_indexes,
        };
        self.add_table(new_config, permit);

        // F-68 (#895) cluster D / task #124: exit timestamp, paired with
        // the "enter" log above.
        log::debug!(
            "rename_table_stores: exit from={from} to={to} elapsed={:?}",
            rts_started.elapsed()
        );

        Ok(true)
    }

    pub async fn repo_identity(&self) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.repo_identity_admitted(&permit).await
    }

    pub(crate) async fn repo_identity_admitted(
        &self,
        permit: &crate::repo::RepoWritePermit,
    ) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        self.write_domain.debug_check_permit(permit);
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        crate::meta::repo_identity::RepoIdentity::load_or_create(&store).await
    }

    pub async fn bump_history_epoch(&self) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.bump_history_epoch_admitted(&permit).await
    }

    pub(crate) async fn bump_history_epoch_admitted(
        &self,
        permit: &crate::repo::RepoWritePermit,
    ) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        self.write_domain.debug_check_permit(permit);
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        crate::meta::repo_identity::RepoIdentity::bump(&store, None).await
    }

    /// Retry-safe even when a previous open bumped before marker removal.
    pub async fn apply_restore_operation(
        &self,
        operation: [u8; 16],
    ) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.apply_restore_operation_admitted(operation, &permit)
            .await
    }

    pub(crate) async fn apply_restore_operation_admitted(
        &self,
        operation: [u8; 16],
        permit: &crate::repo::RepoWritePermit,
    ) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        self.write_domain.debug_check_permit(permit);
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        crate::meta::repo_identity::RepoIdentity::bump(&store, Some(operation)).await
    }
}
