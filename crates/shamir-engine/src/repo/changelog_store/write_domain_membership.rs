//! S5c-X PR6b-2a: admitted table-membership twins on [`RepoInstance`].
//!
//! Table membership changes (`add_table` / `remove_table`) are repository
//! mutations, so they must run under the repository's write-admission
//! permit. The `#[path]`-wired impl-block file keeps `repo_instance.rs`
//! under the 1000-line ratchet; the sync bodies stay there untouched and
//! are simply delegated to after the permit's identity is checked.

use std::sync::Arc;

use shamir_storage::error::DbResult;

use super::super::repo_instance::RepoInstance;
use super::write_domain::RepoWritePermit;
use crate::table::TableConfig;

impl RepoInstance {
    /// Admitted twin of the sync `add_table`: verify the caller holds THIS
    /// repository's write-admission permit, then perform the (synchronous)
    /// membership registration unchanged.
    // No internal await today; PR6b-2c moves admitted store I/O behind this
    // gate, so the async signature is deliberate.
    #[allow(clippy::unused_async)]
    pub(crate) async fn add_table_admitted(
        &self,
        config: TableConfig,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        self.add_table(config);
        Ok(())
    }

    /// Admitted twin of the sync `remove_table`: verify the permit, then
    /// perform the removal. Returns whether the table existed.
    // No internal await today; PR6b-2c moves admitted store I/O behind this
    // gate, so the async signature is deliberate.
    #[allow(clippy::unused_async)]
    pub(crate) async fn remove_table_admitted(
        &self,
        table_name: &str,
        permit: &RepoWritePermit,
    ) -> DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        Ok(self.remove_table(table_name))
    }
}
