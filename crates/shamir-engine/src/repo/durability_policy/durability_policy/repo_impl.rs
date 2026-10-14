//! Durability accessors and replication-only page wiring.

use super::*;
use crate::repo::durability_policy::{resolve, DurabilityMetricsSnapshot};

impl RepoInstance {
    #[cfg(test)]
    pub(crate) async fn from_factory_with_test_segment_bytes(
        name: String,
        factory: BoxRepoFactory,
        configs: Vec<TableConfig>,
        policy: DurabilityPolicy,
        segment_bytes: u64,
    ) -> DbResult<Self> {
        let mut instance = Self::from_factory_unrecovered(name, factory, configs).await?;
        instance.disable_background_drainer_for_test();
        instance.set_wal_segment_bytes_for_test(segment_bytes);
        instance.set_durability(policy)?;
        instance.repo_identity().await?;
        instance.recover_v2_inflight().await?;
        Ok(instance)
    }

    /// Retention is checked against the segment size captured for this open.
    pub async fn from_factory_with_durability(
        name: String,
        factory: BoxRepoFactory,
        configs: Vec<TableConfig>,
        policy: DurabilityPolicy,
    ) -> DbResult<Self> {
        #[cfg(test)]
        let fixture = crate::repo::tests::recovery_gap_marker_tests::take_open_fixture(&name);
        #[cfg(test)]
        let instance = match fixture {
            Some(instance) => instance,
            None => Self::from_factory_unrecovered(name, factory, configs).await?,
        };
        #[cfg(not(test))]
        let instance = Self::from_factory_unrecovered(name, factory, configs).await?;
        instance.set_durability(policy)?;
        // Exclusive factory open initializes legacy identity before registry
        // publication. All later cloned handles share marker_write_mutex.
        instance.repo_identity().await?;
        instance.recover_v2_inflight().await?;
        Ok(instance)
    }

    pub(crate) async fn settle_unwritten_version(&self, version: u64) {
        if let Ok(group) = self.wal_group().await {
            group.settle(version);
        }
    }

    pub(crate) fn initialized_mem_wal(&self) -> bool {
        self.wal_group
            .get()
            .is_some_and(|group| group.is_mem_sink())
    }

    /// PR1 Chunk B, amendment B2: this repo's durability SHARED cell.
    /// Read with `.load()` (one atomic load — the commit seam does exactly
    /// one per commit).
    pub(crate) fn durability(&self) -> &Arc<arc_swap::ArcSwap<DurabilityPolicy>> {
        &self.durability
    }

    /// Scrape initialized WAL and repo-owned pull observations; never opens stores.
    #[must_use]
    pub fn durability_metrics_snapshot(&self) -> DurabilityMetricsSnapshot {
        let mut snapshot = self.durability_metrics.snapshot(
            &self.durability.load(),
            self.wal_group.get().map(Arc::as_ref),
        );
        let retention = self.wal_retention_metrics();
        snapshot.journal = [
            retention[0],
            retention[1],
            retention[2],
            retention[3],
            retention[4],
            self.journal_dropped_total(),
        ];
        snapshot
    }

    /// Snapshot read of the shared policy; pins the value against concurrent stores.
    #[must_use]
    pub fn durability_policy(&self) -> arc_swap::Guard<Arc<DurabilityPolicy>> {
        self.durability.load()
    }

    /// Validate before storing; rejected policies leave every clone unchanged.
    /// Factory opens validate once and install before recovery/admission.
    /// Later setters use the same captured segment size as WAL rotation.
    pub fn set_durability(&self, policy: DurabilityPolicy) -> DbResult<()> {
        let policy =
            resolve(&policy, None, self.wal_segment_bytes)?.for_wal_backend(self.wal_dir.is_some());
        self.durability.store(Arc::new(policy));
        Ok(())
    }

    /// The repo's `WalGroupCommit` (captured when `repo_wal` initializes).
    /// Used by the replication read path to sample `R` and coalesce syncs.
    /// Also exposes the recoverability watermark to embedded callers.
    pub async fn wal_group(&self) -> DbResult<Arc<shamir_wal::WalGroupCommit>> {
        self.repo_wal().await?;
        self.wal_group
            .get()
            .cloned()
            .ok_or_else(|| DbError::Internal("wal group not initialized".to_string()))
    }

    /// One replication journal page, bounded by `min(confirm_through, W, R)`.
    ///
    /// Smallest-correct mechanism: the completion-watermark bound lives
    /// inside [`read_changelog_bounded_impl`](Self::read_changelog_bounded_impl)
    /// and `read_changelog_from` (used by changefeed subscribers) must NOT
    /// gain the `R` clamp — so the replication path calls the bounded impl
    /// directly, once unclamped, and re-reads ONCE with the clamped bound
    /// only when the helper (which may fsync + bounded-wait to raise `R`)
    /// pulled the bound below the first page's. When the first page already
    /// fits under `R` (the common case, incl. every `Mem` WAL) there is no
    /// re-read and no fsync.
    pub(super) async fn read_changelog_replication_page(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<crate::repo::BoundedJournalRead> {
        crate::repo::durability_policy::read_replication_page(
            self,
            from_version,
            limit,
            &self.durability_metrics,
        )
        .await
    }
}
