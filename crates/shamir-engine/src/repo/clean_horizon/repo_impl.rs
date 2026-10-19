use shamir_storage::error::DbResult;
#[cfg(all(test, feature = "fjall"))]
use shamir_storage::types::Repo;
use shamir_storage::types::Store;
use std::sync::Arc;

use super::RepoInstance;
#[cfg(test)]
use crate::tx::clean_horizon::checkpoint::test_seam;

impl RepoInstance {
    #[cfg(all(test, feature = "fjall"))]
    pub(crate) async fn changelog_store_for_test(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__changelog__").await
    }

    #[cfg(all(test, feature = "fjall"))]
    pub(crate) fn set_wal_segment_bytes_for_test(&mut self, bytes: u64) {
        self.wal_segment_bytes = bytes;
    }

    #[cfg(all(test, feature = "fjall"))]
    pub(crate) fn disable_background_drainer_for_test(&self) {
        self.background_drainer_disabled
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn clean_horizon_test_state(
        &self,
    ) -> &std::sync::OnceLock<Arc<std::sync::Mutex<test_seam::State>>> {
        &self.clean_horizon_test_state
    }

    pub(crate) fn wal_retention_gate(
        &self,
    ) -> &Arc<crate::tx::clean_horizon::retention_gate::RetentionGate> {
        &self.wal_retention_gate
    }

    pub(crate) fn journal_dropped_total(&self) -> u64 {
        self.changefeed
            .get()
            .map_or(0, |handle| handle.feed.journal_dropped())
    }

    #[inline]
    pub(crate) fn wal_retention_refuse_writes(&self) -> bool {
        self.wal_retention_gate.refuse_writes()
    }

    pub fn wal_retention_metrics(&self) -> [u64; 5] {
        self.wal_retention_gate.snapshot()
    }

    pub(crate) async fn reserve_recovery_floor(
        &self,
        info: &Arc<dyn Store>,
        minimum: u64,
    ) -> DbResult<u64> {
        let _guard = self.marker_write_mutex().lock().await;
        let current = crate::meta::recovery_marker::load_last_committed(info)
            .await?
            .unwrap_or(0);
        let floor = current.max(minimum);
        crate::meta::recovery_marker::save_last_committed_flushed(info, floor).await?;
        Ok(floor)
    }
}
