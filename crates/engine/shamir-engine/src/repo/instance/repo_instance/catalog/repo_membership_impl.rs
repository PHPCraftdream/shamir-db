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
        self.core.table_by_token_admitted(token, permit).await
    }

    /// Trusted embedding: the returned raw table handle does not authorize actors.
    ///
    /// cancel-safe: yes — uses `OnceCell::get_or_try_init`. On cancel
    /// inside the init closure, the cell remains empty so subsequent
    /// calls retry; no partial table is exposed. The clone of the cell
    /// value is in-memory.
    pub async fn get_table(&self, table_name: &str) -> DbResult<TableManager> {
        self.check_table_rename_fence(table_name)?;
        if let Some(table) = self.core.catalog.initialized_table(table_name) {
            return Ok(table);
        }
        let permit = self
            .core
            .durability_handle
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.check_table_rename_fence(table_name)?;
        // S5c-X Round-12 (stack layout): `get_table_ctx` embeds the whole
        // cold-init chain inline (`create_table_context` →
        // `create_with_domain_admitted` → store_get/tx_gate/interner/MVCC).
        // Pinning it — exactly like the admitted twin below — keeps that
        // await state on the heap instead of widening this fn's future,
        // which is embedded inline in every catalogue read/write along the
        // main-thread admin (ALTER SUBSCRIPTION ... RESUME) poll chain.
        Box::pin(self.core.get_table_ctx(table_name, &permit)).await
    }

    /// Resolve a table during frozen-source token lookups without checking the name fence.
    ///
    /// Read-only repo-wide walkers (FK reverse-cache scan, shutdown flush,
    /// seed walks) use this while a rename is in doubt; the fenced accessors
    /// (`get_table`/`get_table_admitted`) stay for anything that mutates or
    /// resolves a name for a user request. `pub` (not `pub(crate)`) because
    /// the seed-catalogue walker lives in the downstream `shamir-db` crate.
    pub async fn get_table_unfenced(&self, table_name: &str) -> DbResult<TableManager> {
        self.core.get_table_unfenced(table_name).await
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
        self.core
            .durability_handle
            .write_domain
            .debug_check_permit(permit);
        self.check_table_rename_fence(table_name)?;
        Box::pin(self.core.get_table_ctx(table_name, permit)).await
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
        self.core
            .durability_handle
            .write_domain
            .debug_check_permit(permit);
        self.add_table_inner(config);
    }

    fn add_table_inner(&self, config: TableConfig) {
        // Same synchronous DDL lock order as remove_table. This closes the
        // config/reverse-token re-add window without holding guards over await.
        let _entry = self.core.catalog.tables.entry(config.name.clone());
        crate::repo::version_provider::register_token(&self.core.catalog.token_names, &config.name);
        self.core
            .catalog
            .configs
            .insert(config.name.clone(), config);
        self.core.fk_reverse_cache.invalidate();
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
        self.core
            .durability_handle
            .write_domain
            .debug_check_permit(permit);
        self.remove_table_inner(table_name)
    }

    fn remove_table_inner(&self, table_name: &str) -> bool {
        // DDL-only synchronous critical section: tables -> configs -> scc.
        // Keep the table shard exclusive until owned MVCC cleanup completes,
        // so a replacement initializer cannot publish in the removal window.
        let entry = self.core.catalog.tables.entry(table_name.to_string());
        let removed = self.core.catalog.configs.remove(table_name).is_some();
        if removed {
            self.core.fk_reverse_cache.invalidate();
            let token = table_token_for(table_name);
            let _ = self
                .core
                .catalog
                .token_names
                .remove_if_sync(&token, |existing| existing.as_str() == table_name);
            if let dashmap::mapref::entry::Entry::Occupied(entry) = &entry {
                if let Some(mvcc) = entry.get().get().and_then(TableManager::mvcc_store) {
                    let _ = self
                        .core
                        .catalog
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
