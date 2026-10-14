//! Durability accessors and replication-only page wiring.

use super::*;
use crate::repo::durability_policy::{resolve, DurabilityMetricsSnapshot};
use shamir_query_types::{DurabilityOverride, ReplicationServe};

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
        let resolved = resolve(&policy, None, self.wal_segment_bytes)?;
        // Backend-combination validation. This is a COLD admin path — one
        // call per open or per admin operation — never a hot path, so plain
        // branches are fine.
        //
        // `completed_reseed` on an ephemeral backend (in-memory / hybrid;
        // Mem WAL sink, `!has_persistent_wal()`) is ALLOWED: it is the only
        // honest serve mode when nothing survives a restart. `durable`
        // serving on an ephemeral backend is NOT rejected here (durable mem
        // repos are relied on by tests and legal at the engine layer); that
        // combination is validated at publication time, server-side.
        if resolved.effective_wal_commit() == shamir_query_types::WalCommitTier::Synced
            && !self.has_persistent_wal()
        {
            // Loud rejection instead of `for_wal_backend`'s silent
            // downgrade: an explicit admin request for Synced on a backend
            // with no persistent WAL is a configuration error, and the
            // resolved-before-forcing value is what the operator asked for.
            return Err(DbError::Storage(format!(
                "durability.wal_commit = synced requires a file-backed WAL, \
                 but the repo backend is ephemeral (no persistent WAL); \
                 requested tier {:?} would be silently downgraded to buffered",
                resolved.wal_commit(),
            )));
        }
        let policy = resolved.for_wal_backend(self.wal_dir.is_some());
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

    /// One replication journal page. `durable` (default) bounds it by
    /// `min(confirm_through, W, R)`; `completed_reseed` bounds it by
    /// `min(confirm_through, W)` with NO `R` clamp (see
    /// [`read_replication_page`]).
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
        // PR8 completed_reseed chunk 2: arm the lifecycle record BEFORE any
        // speculative page byte is built or returned. The policy is snapshotted
        // into a local `Copy` BEFORE any await so the arc_swap Guard never
        // crosses `.await`; the ensure_armed flush lands strictly before the
        // first served page, so a crash after serving implies Armed on disk.
        let mode: ReplicationServe = self.durability_policy().replication_serve();
        if mode == ReplicationServe::CompletedReseed {
            self.ensure_replication_armed().await?;
        }
        crate::repo::durability_policy::read_replication_page(
            self,
            from_version,
            limit,
            mode,
            &self.durability_metrics,
        )
        .await
    }

    /// Arm the lifecycle record for speculative serving. Atomic fast path
    /// (cache hit), store write at most once per open (cold path): concurrent
    /// clones that miss the cache may both write `Armed`, but the value is
    /// identical and idempotent, so no ordering hazard exists.
    pub(crate) async fn ensure_replication_armed(&self) -> DbResult<()> {
        if self
            .replication_armed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        let store = self.tx_info_store().await?;
        crate::meta::lifecycle_record::ensure_armed(&store).await?;
        self.replication_armed
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Clean lifecycle certificate for `completed_reseed` serving. Called by
    /// `shutdown_clean` AFTER record_clean_horizon / settle / flush_buffers —
    /// the certificate therefore implies journal + allocation + data + WAL
    /// evidence are durable. No-op under `durable` serving (nothing to close).
    pub(crate) async fn write_clean_certificate_if_armed_mode(&self) -> DbResult<()> {
        let mode: ReplicationServe = self.durability_policy().replication_serve();
        if mode != ReplicationServe::CompletedReseed {
            return Ok(());
        }
        let store = self.tx_info_store().await?;
        crate::meta::lifecycle_record::mark_clean(&store).await
    }

    /// Open-time unclean bump (called from `tx::recovery` AFTER
    /// `certify_cold_start`): under `CompletedReseed` serving, an Armed
    /// lifecycle record with no later Clean certificate costs a history-epoch
    /// bump — followers must re-seed. The armed record is left as-is: the
    /// obligation persists until the next clean shutdown. Under `durable`
    /// mode the record is inert (a stale Armed from an earlier fast period is
    /// deliberately NOT erased). A failed bump/flush propagates and blocks
    /// admission (`?` at the caller).
    pub(crate) async fn note_unclean_open_bump(&self) -> DbResult<()> {
        let mode: ReplicationServe = self.durability_policy().replication_serve();
        let store = self.tx_info_store().await?;
        let record = crate::meta::lifecycle_record::load_state(&store).await?;
        if !crate::meta::lifecycle_record::requires_unclean_bump(mode, record) {
            return Ok(());
        }
        // Negative-control seam: constant-false in production, branch elided.
        if cfg!(test)
            && crate::meta::lifecycle_record::SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        self.bump_history_epoch().await?;
        self.durability_metrics.note_epoch_bump();
        Ok(())
    }
}
