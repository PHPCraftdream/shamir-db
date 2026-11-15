use super::super::super::RepoInstance;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use std::sync::Arc;

impl RepoInstance {
    pub(crate) async fn changelog_store_for_cold_reconcile(&self) -> DbResult<Arc<dyn Store>> {
        self.core.changelog_store_for_cold_reconcile().await
    }

    /// Backend capability: does this repo persist a WAL across reopen?
    /// Read on the cold publication path only (replication DDL enforcement).
    pub fn has_persistent_wal(&self) -> bool {
        self.core.has_persistent_wal()
    }

    /// Reconcile the persistent journal from surviving WAL entries before service.
    /// The caller's write-domain permit is threaded through: cold recovery must
    /// never re-admit under a held permit (non-reentrant domain).
    pub(crate) async fn certify_cold_start(
        &self,
        entries: &[shamir_wal::WalEntryV2],
        permit: &crate::repo::RepoWritePermit,
    ) -> DbResult<()> {
        crate::tx::cold_reconcile::reconcile_cold_start(self, entries, permit).await
    }

    /// Legacy certification retained only for backends without a persistent
    /// WAL (exclusive-open certification and coverage installation, never
    /// live replay).
    pub(crate) async fn certify_legacy_cold_start(&self) -> DbResult<()> {
        self.core.certify_legacy_cold_start().await
    }
}
