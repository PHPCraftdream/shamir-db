use crate::table::TableManager;
use shamir_storage::error::DbResult;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;

use super::RepoInstance;

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
