use super::super::super::RepoInstance;
#[cfg(test)]
use shamir_storage::error::DbResult;
#[cfg(test)]
use shamir_storage::types::Store;
#[cfg(test)]
use std::sync::Arc;

impl RepoInstance {
    #[cfg(test)]
    pub(crate) async fn changelog_store_for_cold_reconcile(&self) -> DbResult<Arc<dyn Store>> {
        self.core.changelog_store_for_cold_reconcile().await
    }

    /// Backend capability: does this repo persist a WAL across reopen?
    /// Read on the cold publication path only (replication DDL enforcement).
    pub fn has_persistent_wal(&self) -> bool {
        self.core.has_persistent_wal()
    }

    #[cfg(test)]
    /// Reconcile the persistent journal from surviving WAL entries before service.
    /// The caller's write-domain permit is threaded through: cold recovery must
    /// never re-admit under a held permit (non-reentrant domain).
    pub(crate) async fn certify_cold_start(
        &self,
        entries: &[shamir_wal::WalEntryV2],
        permit: &crate::repo::RepoWritePermit,
    ) -> DbResult<()> {
        self.core.certify_cold_start(entries, permit).await
    }
}
