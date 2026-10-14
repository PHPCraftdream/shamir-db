use super::RepoInstance;
use shamir_storage::error::DbResult;
use shamir_storage::types::{Repo as _, Store};
use std::sync::Arc;

impl RepoInstance {
    pub(crate) async fn changelog_store_for_cold_reconcile(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__changelog__").await
    }

    pub(crate) fn has_persistent_wal(&self) -> bool {
        self.wal_dir.is_some()
    }

    /// Reconcile the persistent journal from surviving WAL entries before service.
    pub(crate) async fn certify_cold_start(
        &self,
        entries: &[shamir_wal::WalEntryV2],
    ) -> DbResult<()> {
        crate::tx::cold_reconcile::reconcile_cold_start(self, entries).await
    }

    pub(crate) async fn certify_legacy_cold_start(&self) -> DbResult<()> {
        crate::repo::durability_policy::certify_recovered_open(self).await
    }
}
