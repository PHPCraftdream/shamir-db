use shamir_storage::error::DbResult;
#[cfg(test)]
#[cfg(feature = "fjall")]
use shamir_storage::types::Repo;
use shamir_storage::types::Store;

use crate::repo::journal::backend::durability_handle::retention_gate::RetentionGate;
use std::sync::Arc;

use super::super::RepoInstance;
#[cfg(test)]
use crate::tx::clean_horizon::checkpoint::test_seam;

impl RepoInstance {
    #[cfg(all(test, feature = "fjall"))]
    pub(crate) async fn changelog_store_for_test(&self) -> DbResult<Arc<dyn Store>> {
        self.core.repo.store_get("__changelog__").await
    }

    #[cfg(all(test, feature = "fjall"))]
    pub(crate) fn set_wal_segment_bytes_for_test(&mut self, bytes: u64) {
        self.core.wal_segment_bytes = bytes;
    }

    #[cfg(test)]
    pub(crate) fn disable_background_drainer_for_test(&self) {
        self.core
            .background_drainer_disabled
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn clean_horizon_test_state(
        &self,
    ) -> &std::sync::OnceLock<Arc<std::sync::Mutex<test_seam::State>>> {
        &self.core.clean_horizon_test_state
    }

    pub(crate) fn wal_retention_gate(&self) -> &Arc<RetentionGate> {
        self.core.durability_handle.wal_retention_gate()
    }

    #[inline]
    pub(crate) fn wal_retention_refuse_writes(&self) -> bool {
        self.core.durability_handle.wal_retention_refuse_writes()
    }

    pub fn wal_retention_metrics(&self) -> [u64; 5] {
        self.core.durability_handle.wal_retention_metrics()
    }

    pub(crate) async fn reserve_recovery_floor(
        &self,
        info: &Arc<dyn Store>,
        minimum: u64,
    ) -> DbResult<u64> {
        self.core
            .durability_handle
            .reserve_recovery_floor(info, minimum)
            .await
    }
}
