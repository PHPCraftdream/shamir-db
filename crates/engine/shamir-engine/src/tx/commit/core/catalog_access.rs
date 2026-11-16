//! Catalogue-resolution methods of `RepoCore` (R2c1b worker 2): token→name
//! resolution, table open (fenced / unfenced / admitted) and the cold-init
//! context builder. The `RepoInstance` shell keeps one-line delegates.

use super::repo_core::RepoCore;
use crate::repo::changelog_store::write_domain::RepoWritePermit;
use crate::repo::fk_reverse_cache::FkReverseCache;
use crate::table::table_manager::table_token_for;
use crate::table::{table_store_names, TableManager};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{fully_unwrap_store, Repo as _, Store};
use std::sync::Arc;
use tokio::sync::OnceCell;

impl RepoCore {
    /// Look up the table whose token matches `token`. Used by V2 WAL
    /// recovery to resolve ops by `table_id_interned`, and by the commit
    /// pipeline (Phases 1, 2.6, 5b–5d) while holding the per-repo
    /// `commit_lock`.
    ///
    /// cancel-safe: yes — one `scc::HashMap` read plus a single
    /// `get_table_unfenced` call. Cancellation leaves no state mutated
    /// (apart from the cancel-safe `get_table_unfenced` OnceCell init).
    ///
    /// O(1) (III.1): resolves through the `token_names` reverse index that
    /// `add_table` maintains, instead of scanning every config and
    /// re-hashing its name. The old O(N) scan grew the serialized
    /// critical section linearly with schema size. `add_table` and
    /// `remove_table` keep `token_names` in lock-step with `configs`, so a
    /// resolved name always still has a config; the init closure
    /// re-validates against `configs` regardless.
    ///
    /// Since P34-03d the resolution goes through the unfenced accessor
    /// (`get_table_unfenced`) so a fenced in-doubt source still resolves;
    /// this replaced the old `get_table` call deliberately.
    ///
    /// Resolve a registered table token, initializing its table manager if needed.
    ///
    /// This lookup is intentionally unfenced: callers such as transaction prelock
    /// must reach the manager to observe a rename freeze, while fencing this lookup
    /// would make the drainer wait on work that cannot discover the freeze.
    pub(crate) async fn table_by_token(&self, token: u64) -> DbResult<Option<TableManager>> {
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
        match self
            .catalog
            .token_names
            .read_sync(&token, |_, name| name.clone())
        {
            Some(name) => self.get_table_unfenced(&name).await.map(Some),
            None => Ok(None),
        }
    }

    /// Resolve a token while the caller already holds this repository's permit.
    pub(crate) async fn table_by_token_admitted(
        &self,
        token: u64,
        permit: &RepoWritePermit,
    ) -> DbResult<Option<TableManager>> {
        self.durability_handle
            .write_domain
            .debug_check_permit(permit);
        let name = self
            .catalog
            .token_names
            .read_sync(&token, |_, name| name.clone());
        match name {
            Some(name) => self
                .get_table_admitted_unfenced(&name, permit)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    /// Catalogue read; see `TableCatalog`.
    pub(crate) async fn table_by_token_if_live(&self, token: u64) -> Option<TableManager> {
        self.catalog.table_by_token_if_live(token).await
    }

    /// Trusted embedding: the returned raw table handle does not authorize actors.
    ///
    /// cancel-safe: yes — uses `OnceCell::get_or_try_init`. On cancel
    /// inside the init closure, the cell remains empty so subsequent
    /// calls retry; no partial table is exposed. The clone of the cell
    /// value is in-memory.
    pub(crate) async fn get_table(&self, table_name: &str) -> DbResult<TableManager> {
        self.rename_fence.check_table_rename_fence(table_name)?;
        if let Some(table) = self.catalog.initialized_table(table_name) {
            return Ok(table);
        }
        let permit = self
            .durability_handle
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.rename_fence.check_table_rename_fence(table_name)?;
        // S5c-X Round-12 (stack layout): `get_table_ctx` embeds the whole
        // cold-init chain inline (`create_table_context` →
        // `create_with_domain_admitted` → store_get/tx_gate/interner/MVCC).
        // Pinning it — exactly like the admitted twin below — keeps that
        // await state on the heap instead of widening this fn's future,
        // which is embedded inline in every catalogue read/write along the
        // main-thread admin (ALTER SUBSCRIPTION ... RESUME) poll chain.
        Box::pin(self.get_table_ctx(table_name, &permit)).await
    }

    /// Admitted twin of `get_table`: requires the caller to hold THIS
    /// repository's write-admission permit; the permit is checked
    /// (`debug_check_permit`) and threaded into table construction. Otherwise
    /// identical semantics to `get_table`.
    pub(crate) async fn get_table_admitted(
        &self,
        table_name: &str,
        permit: &RepoWritePermit,
    ) -> DbResult<TableManager> {
        self.durability_handle
            .write_domain
            .debug_check_permit(permit);
        self.rename_fence.check_table_rename_fence(table_name)?;
        Box::pin(self.get_table_ctx(table_name, permit)).await
    }

    /// Trusted embedding: the returned raw table handle does not authorize actors.
    ///
    /// cancel-safe: yes — uses `OnceCell::get_or_try_init`. On cancel
    /// inside the init closure, the cell remains empty so subsequent
    /// calls retry; no partial table is exposed. The clone of the cell
    /// value is in-memory.
    pub(crate) async fn get_table_unfenced(&self, table_name: &str) -> DbResult<TableManager> {
        if let Some(table) = self.catalog.initialized_table(table_name) {
            return Ok(table);
        }
        let permit = self
            .durability_handle
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        Box::pin(self.get_table_ctx(table_name, &permit)).await
    }

    /// Admitted twin of `get_table`: requires the caller to
    /// hold THIS repository's write-admission permit; the permit is checked
    /// (`debug_check_permit`) and threaded into table construction so the
    /// produced `TableManager` is born already holding an admitted
    /// capability. Otherwise identical semantics to `get_table` (single-init
    /// `OnceCell`, existence checks inside the init closure).
    pub(crate) async fn get_table_admitted_unfenced(
        &self,
        table_name: &str,
        permit: &RepoWritePermit,
    ) -> DbResult<TableManager> {
        self.durability_handle
            .write_domain
            .debug_check_permit(permit);
        Box::pin(self.get_table_ctx(table_name, permit)).await
    }

    pub(crate) async fn get_table_ctx(
        &self,
        table_name: &str,
        permit: &RepoWritePermit,
    ) -> DbResult<TableManager> {
        // Clone the `Arc<OnceCell>` out of the DashMap and DROP the shard
        // guard BEFORE the init `.await`. DashMap shards are backed by a
        // synchronous `RwLock`; holding the `entry()` write guard across the
        // long-running `create_table_context().await` (which itself awaits
        // store_get / tx_gate / changefeed init) is a guard-across-await
        // deadlock: under runtime oversubscription every worker thread can
        // become wedged on the OS `RwLock` of a shard whose guard-holder is
        // parked at an `.await`, and a synchronous lock cannot yield. The
        // `OnceCell` itself provides the single-init serialization — the
        // shard lock only needs to protect the map insert, not the init.
        let cell = Arc::clone(
            self.catalog
                .tables
                .entry(table_name.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .value(),
        );

        // §B13: existence-check happens INSIDE the init closure, so it
        // is serialized with the actual context construction. Doing the
        // check up-front would race with concurrent `remove_table`
        // between our `configs.contains_key` and the `tables.entry`
        // install (two independent DashMaps). On a removed table the
        // init returns Err and `OnceCell::get_or_try_init` leaves the
        // cell empty so subsequent calls retry.
        let tbl = cell
            .get_or_try_init(|| async {
                if !self.catalog.configs.contains_key(table_name) {
                    return Err(DbError::NotFound(format!(
                        "Table '{}' is not configured in this repository",
                        table_name
                    )));
                }
                self.create_table_context(table_name, &cell, permit).await
            })
            .await?
            .clone();

        // Close the insertion-to-OnceCell-completion window against DROP.
        // No map guard survives an await; rollback can only evict our Arc.
        let current = self.catalog.tables.get(table_name);
        if !self.catalog.configs.contains_key(table_name)
            || !current
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry.value(), &cell))
        {
            if let Some(mvcc) = tbl.mvcc_store() {
                let _ = self
                    .catalog
                    .per_table_mvcc
                    .remove_if_sync(&table_token_for(table_name), |existing| {
                        Arc::ptr_eq(existing, &mvcc)
                    });
            }
            return Err(DbError::NotFound(format!(
                "Table '{table_name}' was removed during initialization"
            )));
        }
        Ok(tbl)
    }

    /// cancel-safe: yes — all awaits precede MVCC publication. After insert,
    /// only synchronous ownership checks and builders remain.
    async fn create_table_context(
        &self,
        table_name: &str,
        cell: &Arc<OnceCell<TableManager>>,
        permit: &RepoWritePermit,
    ) -> DbResult<TableManager> {
        let config = self
            .catalog
            .configs
            .get(table_name)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| {
                DbError::NotFound(format!(
                    "Table '{table_name}' was removed during initialization"
                ))
            })?;
        let id = config.store_id;
        let names = table_store_names(table_name, config.store_id.as_ref());
        let data_store: Arc<dyn Store> = self.repo.store_get(names.data).await?;
        let info_store: Arc<dyn Store> = self.repo.store_get(names.info).await?;
        let history_store: Arc<dyn Store> = self.repo.store_get(names.history).await?;

        let gate = self.tx_gate().await?;

        // This seam models a failure/cancellation in setup that used to run
        // AFTER registration. It is per-repo and consumed once, never global.
        #[cfg(test)]
        if let Some(hook) = self.table_init_hook.swap(None) {
            hook.run().await?;
        }

        // Complete ALL asynchronous/fallible setup before publication.
        // __data__ is inert under MVCC; __info__ keeps its wrappers.
        let data_store = fully_unwrap_store(&data_store).await;
        self.durability_handle
            .write_domain
            .debug_check_permit(permit);
        // The MVCC store is handed to the open itself (not registered yet):
        // open-time index rebuilds read the records through it, `__data__` is
        // inert here and would yield no rows.
        let mvcc = Arc::new(shamir_tx::MvccStore::new(history_store, Arc::clone(&gate)));
        if self
            .catalog
            .configs
            .get(table_name)
            .is_none_or(|current| current.value().store_id != id)
        {
            return Err(DbError::NotFound(format!(
                "Table '{table_name}' changed identity during initialization"
            )));
        }

        let tbl = TableManager::open_with_domain_admitted(
            table_name.to_string(),
            data_store,
            info_store,
            Arc::clone(&self.durability_handle.write_domain),
            permit,
            Some(Arc::clone(&mvcc)),
        )
        .await?;
        let repo_interner = self.repo_interner().await?;

        let token = table_token_for(table_name);
        // A13 follow-up: `insert_sync` returns `Err` when the token is
        // ALREADY present — e.g. a `remove_table(name)` racing this cold
        // init can let a second `create_table_context` run concurrently
        // (see `remove_table`'s own A13 doc comment). Silently discarding
        // that `Err` (the old behavior) leaves the STALE old `MvccStore`
        // wired into the commit pipeline forever while this freshly-built
        // `TableManager` reads through its own (orphaned) `mvcc` handle — a
        // split-brain where committed transactions silently vanish. Fail
        // the table open outright instead: a lost race here means the
        // caller's `OnceCell::get_or_try_init` leaves the cell empty and a
        // retry re-attempts the attach cleanly.
        if self
            .catalog
            .per_table_mvcc
            .insert_sync(token, Arc::clone(&mvcc))
            .is_err()
        {
            log::error!(
                "create_table_context: per_table_mvcc already has an entry for \
                 table '{table_name}' (token {token}) — refusing to attach a second \
                 MvccStore, which would leave a stale store wired into the commit \
                 pipeline while this TableManager reads through a fresh one (A13 \
                 split-brain). Failing the table open; retry will re-attempt the \
                 attach."
            );
            return Err(DbError::Internal(format!(
                "create_table_context: per_table_mvcc attach collision for table \
                 '{table_name}' (token {token}) — a stale MvccStore entry is still \
                 registered"
            )));
        }

        // DROP may have removed/replaced our OnceCell while setup awaited.
        // Existence alone is insufficient after DROP + CREATE of the same name.
        let current = self.catalog.tables.get(table_name);
        if !self.catalog.configs.contains_key(table_name)
            || !current
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry.value(), cell))
        {
            let _ = self
                .catalog
                .per_table_mvcc
                .remove_if_sync(&token, |existing| Arc::ptr_eq(existing, &mvcc));
            return Err(DbError::NotFound(format!(
                "Table '{table_name}' was removed during initialization"
            )));
        }
        let tbl = tbl.with_mvcc_store(mvcc).with_interner(repo_interner);

        // Wire the non-tx write path to the per-repo gate so that
        // Serializable transactions see non-tx writes in their Phase 2-bis
        // predicate-conflict check. The table uses the SAME `gate` the tx
        // commit pipeline uses, keeping non-tx and tx `commit_version`s on
        // one monotonic sequence per repo.
        Ok(tbl.with_changefeed(Arc::clone(&gate)))
    }

    /// Catalogue read; see `TableCatalog`.
    pub(crate) fn has_table(&self, name: &str) -> bool {
        self.catalog.has_table(name)
    }

    /// Catalogue read; see `TableCatalog`.
    pub(crate) fn list_table_names(&self) -> Vec<String> {
        self.catalog.list_table_names()
    }

    /// Catalogue read; see `TableCatalog`.
    pub(crate) fn mapped_table_store_names(&self, name: &str) -> Option<(String, String, String)> {
        self.catalog.mapped_table_store_names(name)
    }

    /// F-28 Step 4 (#831): this repo's cached reverse-FK map. See
    /// `FkReverseCache` for the cache-aside population + invalidation
    /// contract. Used by `query::batch`'s FK discovery functions (via
    /// `BatchResolver::resolve_repo`) and by the shell's
    /// `RepoInstance::fk_reverse_cache` delegate.
    pub(crate) fn fk_reverse_cache(&self) -> &Arc<FkReverseCache> {
        &self.fk_reverse_cache
    }
}
