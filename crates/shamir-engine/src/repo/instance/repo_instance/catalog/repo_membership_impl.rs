//! S5c-X PR6b-2a/2c: table-membership + table-open impl block, moved out of
//! `repo_instance.rs` for the line ratchet.
//!
//! This file is wired as a DESCENDANT of the `repo_instance` module
//! (declared by `repo_instance/catalog.rs`), so the private `RepoInstance`
//! fields (`tables`, `configs`, `token_names`, `fk_reverse_cache`, ...) and
//! the free fn `register_token` remain reachable.

use std::sync::Arc;

use crate::repo::changelog_store::write_domain::RepoWritePermit;
use crate::table::table_manager::table_token_for;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{fully_unwrap_store, Repo, Store};
use tokio::sync::OnceCell;

use super::super::RepoInstance;

impl RepoInstance {
    #[cfg(test)]
    pub(crate) fn add_table_for_test(&self, config: TableConfig) {
        self.add_table_inner(config);
    }

    #[cfg(test)]
    pub(crate) fn remove_table_for_test(&self, table_name: &str) -> bool {
        self.remove_table_inner(table_name)
    }

    /// Resolve a token while the caller already holds this repository's permit.
    pub async fn table_by_token_admitted(
        &self,
        token: u64,
        permit: &RepoWritePermit,
    ) -> DbResult<Option<TableManager>> {
        self.write_domain.debug_check_permit(permit);
        let name = self.token_names.read_sync(&token, |_, name| name.clone());
        match name {
            Some(name) => self.get_table_admitted(&name, permit).await.map(Some),
            None => Ok(None),
        }
    }

    /// Trusted embedding: the returned raw table handle does not authorize actors.
    ///
    /// cancel-safe: yes — uses `OnceCell::get_or_try_init`. On cancel
    /// inside the init closure, the cell remains empty so subsequent
    /// calls retry; no partial table is exposed. The clone of the cell
    /// value is in-memory.
    pub async fn get_table(&self, table_name: &str) -> DbResult<TableManager> {
        if let Some(table) = self.initialized_table(table_name) {
            return Ok(table);
        }
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        // S5c-X Round-12 (stack layout): `get_table_ctx` embeds the whole
        // cold-init chain inline (`create_table_context` →
        // `create_with_domain_admitted` → store_get/tx_gate/interner/MVCC).
        // Pinning it — exactly like the admitted twin below — keeps that
        // await state on the heap instead of widening this fn's future,
        // which is embedded inline in every catalogue read/write along the
        // main-thread admin (ALTER SUBSCRIPTION ... RESUME) poll chain.
        Box::pin(self.get_table_ctx(table_name, &permit)).await
    }

    /// Returns an already initialized table without admitting or triggering
    /// initialization. Callers must not retain a map guard across awaits.
    fn initialized_table(&self, table_name: &str) -> Option<TableManager> {
        let cell = self.tables.get(table_name)?;
        cell.get().cloned()
    }

    /// Admitted twin of [`RepoInstance::get_table`]: requires the caller to
    /// hold THIS repository's write-admission permit; the permit is checked
    /// (`debug_check_permit`) and threaded into table construction so the
    /// produced [`TableManager`] is born already holding an admitted
    /// capability. Otherwise identical semantics to `get_table` (single-init
    /// `OnceCell`, existence checks inside the init closure).
    pub async fn get_table_admitted(
        &self,
        table_name: &str,
        permit: &RepoWritePermit,
    ) -> DbResult<TableManager> {
        self.write_domain.debug_check_permit(permit);
        // S5c-X Round-11 (stack layout): `get_table_ctx` embeds the whole
        // cold-init chain (`create_table_context` →
        // `create_with_domain_admitted`) inline; pinning it keeps that
        // await state on the heap instead of widening every admitted
        // caller's future along the seed-install / replay poll chains.
        Box::pin(self.get_table_ctx(table_name, permit)).await
    }

    async fn get_table_ctx(
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
            self.tables
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
                if !self.configs.contains_key(table_name) {
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
        let current = self.tables.get(table_name);
        if !self.configs.contains_key(table_name)
            || !current
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry.value(), &cell))
        {
            if let Some(mvcc) = tbl.mvcc_store() {
                let _ = self
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
        let data_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__data__{}", table_name))
            .await?;
        let info_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__info__{}", table_name))
            .await?;
        let history_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__history__{}", table_name))
            .await?;

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
        self.write_domain.debug_check_permit(permit);
        // The identity migration is deferred until the MVCC view is attached
        // (below): `__data__` is inert here, so a rebuild now would see no rows.
        let tbl = TableManager::open_with_domain_admitted(
            table_name.to_string(),
            data_store,
            info_store,
            Arc::clone(&self.write_domain),
            permit,
            false,
        )
        .await?;
        let repo_interner = self.repo_interner().await?;
        let mvcc = Arc::new(shamir_tx::MvccStore::new(history_store, Arc::clone(&gate)));

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
        let current = self.tables.get(table_name);
        if !self.configs.contains_key(table_name)
            || !current
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry.value(), cell))
        {
            let _ = self
                .per_table_mvcc
                .remove_if_sync(&token, |existing| Arc::ptr_eq(existing, &mvcc));
            return Err(DbError::NotFound(format!(
                "Table '{table_name}' was removed during initialization"
            )));
        }
        let tbl = tbl
            .with_mvcc_store(Arc::clone(&mvcc))
            .with_interner(repo_interner);
        if let Err(error) = tbl.ensure_base_identity_current(permit).await {
            let _ = self
                .per_table_mvcc
                .remove_if_sync(&token, |existing| Arc::ptr_eq(existing, &mvcc));
            return Err(error);
        }

        // Wire the non-tx write path to the per-repo gate so that
        // Serializable transactions see non-tx writes in their Phase 2-bis
        // predicate-conflict check. The table uses the SAME `gate` the tx
        // commit pipeline uses, keeping non-tx and tx `commit_version`s on
        // one monotonic sequence per repo.
        Ok(tbl.with_changefeed(Arc::clone(&gate)))
    }

    /// Register a new table in the repository.
    /// The table is lazily created on first access via get_table().
    ///
    /// Also records `table_token_for(name) → name` in the reverse index so
    /// `table_by_token` resolves in O(1) (III.1). Re-registering an already
    /// known name is idempotent.
    ///
    /// F-28 Step 4 (#831): invalidates the cached reverse-FK map — a newly
    /// registered table changes `list_table_names()`'s membership, which
    /// `discover_restrict_refs`/`discover_action_refs` scan when rebuilding
    /// the cache.
    ///
    /// S5c-X PR6b-2a: callers must pass a permit for this repository.
    pub(crate) fn add_table(&self, config: TableConfig, permit: &RepoWritePermit) {
        self.write_domain.debug_check_permit(permit);
        self.add_table_inner(config);
    }

    fn add_table_inner(&self, config: TableConfig) {
        // Same synchronous DDL lock order as remove_table. This closes the
        // config/reverse-token re-add window without holding guards over await.
        let _entry = self.tables.entry(config.name.clone());
        crate::repo::version_provider::register_token(&self.token_names, &config.name);
        self.configs.insert(config.name.clone(), config);
        self.fk_reverse_cache.invalidate();
    }

    /// Remove a table from the repository.
    /// Returns true if the table existed and was removed.
    ///
    /// F-28 Step 4 (#831): invalidates the cached reverse-FK map on an
    /// actual removal — same rationale as `add_table` (table membership
    /// changed).
    ///
    /// S5c-X PR6b-2a: callers must pass a permit for this repository.
    pub(crate) fn remove_table(&self, table_name: &str, permit: &RepoWritePermit) -> bool {
        self.write_domain.debug_check_permit(permit);
        self.remove_table_inner(table_name)
    }

    fn remove_table_inner(&self, table_name: &str) -> bool {
        // DDL-only synchronous critical section: tables -> configs -> scc.
        // Keep the table shard exclusive until owned MVCC cleanup completes,
        // so a replacement initializer cannot publish in the removal window.
        let entry = self.tables.entry(table_name.to_string());
        let removed = self.configs.remove(table_name).is_some();
        if removed {
            self.fk_reverse_cache.invalidate();
            let token = table_token_for(table_name);
            let _ = self
                .token_names
                .remove_if_sync(&token, |existing| existing.as_str() == table_name);
            if let dashmap::mapref::entry::Entry::Occupied(entry) = &entry {
                if let Some(mvcc) = entry.get().get().and_then(TableManager::mvcc_store) {
                    let _ = self
                        .per_table_mvcc
                        .remove_if_sync(&token, |existing| Arc::ptr_eq(existing, &mvcc));
                }
            }
            // An empty/in-flight cell owns no published MVCC. Its initializer
            // must detect retirement and roll back only its own registration.
            if let dashmap::mapref::entry::Entry::Occupied(entry) = entry {
                entry.remove();
            }
        }
        removed
    }
}
