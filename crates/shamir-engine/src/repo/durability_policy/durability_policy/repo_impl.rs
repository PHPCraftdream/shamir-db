//! Durability accessors and replication-only page wiring.

use super::*;
use crate::repo::durability_policy::{resolve, DurabilityMetricsSnapshot};
use shamir_query_types::DurabilityOverride;

impl RepoInstance {
    /// Open with a validated policy installed before identity and WAL recovery.
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
    /// one per commit); retention fields are consumed in PR2.
    pub(crate) fn durability(&self) -> &Arc<arc_swap::ArcSwap<DurabilityPolicy>> {
        &self.durability
    }

    /// Scrape initialized WAL and repo-owned pull observations; never opens stores.
    #[must_use]
    pub fn durability_metrics_snapshot(&self) -> DurabilityMetricsSnapshot {
        self.durability_metrics.snapshot(
            &self.durability.load(),
            self.wal_group.get().map(Arc::as_ref),
        )
    }

    /// Snapshot read of the shared policy; pins the value against concurrent stores.
    #[must_use]
    pub fn durability_policy(&self) -> arc_swap::Guard<Arc<DurabilityPolicy>> {
        self.durability.load()
    }

    /// Check whether an already-initialized transaction gate requires repo
    /// reopen, without initializing the gate or touching transaction state.
    pub fn durability_change_requires_reopen(&self) -> bool {
        self.initialized_tx_gate()
            .is_some_and(shamir_tx::RepoTxGate::requires_reopen)
    }

    /// Resolve and validate an ALTER override against this open repo's
    /// captured segment size, without initializing transaction state. The
    /// result can be passed to `set_durability` after catalogue persistence;
    /// both paths use the same immutable segment-size input.
    pub fn validate_durability_change(
        &self,
        server_default: &DurabilityPolicy,
        over: &DurabilityOverride,
    ) -> DbResult<DurabilityPolicy> {
        if self.durability_change_requires_reopen() {
            return Err(DbError::RepoRequiresReopen(self.name.clone()));
        }
        resolve(server_default, Some(over), self.wal_segment_bytes)
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
