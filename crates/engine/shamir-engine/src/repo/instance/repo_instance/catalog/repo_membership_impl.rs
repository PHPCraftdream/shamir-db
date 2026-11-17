//! S5c-X PR6b-2a/2c: table-membership + table-open impl block, moved out of
//! `repo_instance.rs` for the line ratchet.
//!
//! This file is wired as a descendant of `repo_instance`; membership state is
//! mutated through `TableCatalog` while FK invalidation stays on `RepoCore`.

use crate::repo::changelog_store::write_domain::RepoWritePermit;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;

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
        self.core.get_table(table_name).await
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
        self.core.get_table_admitted(table_name, permit).await
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
        self.core.catalog.register_table_with(config, || {
            self.core.fk_reverse_cache.invalidate();
        });
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
        self.core.catalog.remove_table_with(table_name, || {
            self.core.fk_reverse_cache.invalidate();
        })
    }
}
