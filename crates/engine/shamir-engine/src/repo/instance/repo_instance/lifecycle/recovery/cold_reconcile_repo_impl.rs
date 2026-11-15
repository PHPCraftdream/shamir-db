use super::super::super::RepoInstance;
use shamir_storage::error::DbResult;
use shamir_storage::types::{Repo as _, Store};
use std::sync::Arc;

impl RepoInstance {
    pub(crate) async fn changelog_store_for_cold_reconcile(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__changelog__").await
    }

    /// Backend capability: does this repo persist a WAL across reopen?
    /// Read on the cold publication path only (replication DDL enforcement).
    pub fn has_persistent_wal(&self) -> bool {
        self.durability_handle.has_persistent_wal()
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
        let gate = self.tx_gate().await?;
        let h = gate.version_allocation_high_water_mark();
        self.flush_all_history().await?;
        let handle = self.changefeed().await?;
        let j = handle.feed.clean_horizon();
        if h > j {
            handle.feed.record_recovery_gap(j + 1, h).await?;
            handle.feed.record_clean_horizon(h).await?;
        }
        // Only AFTER every recovery/certification I/O succeeded. Missing WAL
        // predecessors may be aborted, in durable history, or unknown: the gap
        // prevents serving them as a complete journal, while R permits new writes.
        if let Some(coverage) = self.wal_group().await?.coverage_observer() {
            coverage.certify_exclusive_open(h);
        }
        Ok(())
    }
}
