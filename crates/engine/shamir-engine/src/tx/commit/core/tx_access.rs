use super::repo_core::RepoCore;
use crate::meta::recovery_marker;
use crate::meta::repl::gate_provenance::with_follower_provenance as provenance_gate;
use crate::repo::RepoWritePermit;
use crate::table::interner_manager::InternerManager;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{AtomicBatch, Repo as _, Store};
use shamir_types::types::common::THasher;
use std::sync::{Arc, Weak};

impl RepoCore {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// cancel-safe: yes — `OnceCell::get_or_try_init` semantics. If
    /// cancellation occurs during init, the cell remains uninitialised
    /// and the next caller retries. Returned Arc clone is in-memory.
    ///
    /// Returns the per-repo transaction gate, lazily initialising it on
    /// first call.
    ///
    /// The gate is seeded from durable recovery markers stored under the
    /// repo's `"__tx__"` info store:
    /// - `last_committed_version` from `MetaKey::LastCommittedVersion`
    /// - `next_tx_id` from `MetaKey::NextTxId`
    ///
    /// Recovery marker reads are best-effort — if absent we start from
    /// defaults (`RepoTxGate::fresh()`).
    ///
    /// CRIT-B: the persisted `last_committed_version` marker can lag the
    /// highest `commit_version` of an *inflight* V2 WAL entry — the
    /// commit pipeline stamps `commit_version` in Phase 4 (WAL begin) but
    /// only persists the marker in Phase 6.5. A crash in that window
    /// leaves the marker stale (e.g. 7) while a durable inflight entry
    /// carries `commit_version = 10`. Seeding the gate solely from the
    /// marker would let `assign_next_version()` re-issue 8, 9, 10 —
    /// versions the crashed (and about-to-be-recovered) tx already
    /// consumed — violating version monotonicity. We therefore pre-scan
    /// the inflight entries and seed the gate's counter from
    /// `max(marker, max_inflight_commit_version)`. `RepoTxGate::new`
    /// initialises BOTH the monotonic `version_counter` and
    /// `last_committed_version` to this floor, so the next
    /// `assign_next_version()` is strictly greater than every version a
    /// recovered entry will replay at. The scan is the same cheap
    /// `recover()` replay recovery runs and happens once per gate (OnceCell).
    pub(crate) async fn tx_gate(&self) -> DbResult<Arc<shamir_tx::RepoTxGate>> {
        self.horizon
            .tx_gate_cell()
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;

                let marker = recovery_marker::load_last_committed(&info_store)
                    .await?
                    .unwrap_or(0);
                let next_tx_id = recovery_marker::load_next_tx_id_snapshot(&info_store)
                    .await?
                    .unwrap_or(1);

                // Pre-scan inflight V2 entries so the version floor covers
                // any commit_version stamped before its marker was
                // persisted (CRIT-B). Best-effort: a WAL read error here
                // falls back to the marker rather than blocking gate
                // construction — recovery (which propagates errors) runs
                // separately on the open path.
                let max_inflight = self.max_inflight_commit_version().await.unwrap_or(0);
                let checkpoint = self.checkpoint_allocation_floor().await?;
                let last_committed = marker.max(max_inflight).max(checkpoint);
                let gate = provenance_gate(last_committed, next_tx_id, &info_store).await?;
                // Best-effort: a WAL-init failure must NOT fail the gate
                // init. The old contract (see commit_wal_init_failure tests)
                // keeps the repo_wal OnceCell uninitialised and retryable —
                // the failure surfaces only at commit_tx's repo_wal() call.
                if let Ok(group) = self.wal_group().await {
                    if let Some(coverage) = group.coverage_observer() {
                        gate.completion().install_abort_coverage(coverage);
                    }
                }
                Ok::<Arc<shamir_tx::RepoTxGate>, DbError>(gate)
            })
            .await
            .cloned()
    }

    pub(crate) fn tx_metrics(&self) -> &Arc<shamir_tx::TxMetrics> {
        self.horizon.tx_metrics()
    }

    pub(crate) fn commit_tail(&self) -> &Arc<crate::tx::CommitTailBarrier> {
        self.horizon.commit_tail()
    }

    pub(crate) fn commit_visibility_wait(&self) -> shamir_query_types::batch::VisibilityWait {
        self.horizon.commit_visibility_wait()
    }

    pub(crate) fn initialized_tx_gate(&self) -> Option<&shamir_tx::RepoTxGate> {
        self.horizon.initialized_tx_gate()
    }

    /// Returns the repo's transactional info_store under the
    /// `"__tx__"` namespace.
    ///
    /// Shared with [`tx_gate`](Self::tx_gate) and
    /// [`repo_wal`](Self::repo_wal). The commit pipeline writes
    /// recovery markers (`MetaKey::LastCommittedVersion`,
    /// `MetaKey::NextTxId`) here in Phase 6.5 (see
    /// `crate::tx::commit`) so a clean restart can seed the gate
    /// without scanning every active WAL marker.
    pub(crate) async fn tx_info_store(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__tx__").await
    }

    /// Highest `commit_version` across all durable inflight V2 WAL
    /// entries, or `0` if there are none. Used to seed the tx gate's
    /// version floor (CRIT-B) so recovered commit versions are never
    /// re-issued.
    pub(crate) async fn max_inflight_commit_version(&self) -> DbResult<u64> {
        let wal = self.repo_wal().await?;
        let entries = wal.recover().await?;
        Ok(entries.iter().map(|e| e.commit_version).max().unwrap_or(0))
    }

    /// Audit §1.7: mutex guarding the recovery-marker disk write. See the
    /// field doc on `RepoInstance::marker_write_mutex` for why this exists.
    pub(crate) fn marker_write_mutex(&self) -> &Arc<tokio::sync::Mutex<()>> {
        &self.durability_handle.marker_write_mutex
    }

    /// Begin a repository-wide atomic batch over named stores (PR6b-2).
    /// The permit is the admitted-API capability: callers must already hold
    /// a `RepoWritePermit` from this repo's write domain; there is NO second
    /// admission mechanism here.
    pub(crate) async fn begin_repo_atomic_batch(
        &self,
        permit: &RepoWritePermit,
    ) -> DbResult<Box<dyn AtomicBatch>> {
        self.durability_handle
            .write_domain
            .debug_check_permit(permit);
        self.repo.begin_atomic_batch().await
    }

    /// Backend capability for the S5c-X PR6b-2 receipt apply protocol.
    pub(crate) fn storage_supports_atomic_batch(&self) -> bool {
        self.repo.supports_atomic_batch()
    }

    /// Per-table MvccStore map used by the commit pipeline to route
    /// data writes through version-aware storage.
    pub(crate) fn per_table_mvcc(
        &self,
    ) -> &Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>> {
        &self.catalog.per_table_mvcc
    }

    /// cancel-safe: yes — `OnceCell::get_or_try_init` semantics. If
    /// cancellation occurs during init the cell stays empty and the next
    /// caller retries. Returned manager is a cheap clone (all internal Arcs
    /// are shared).
    ///
    /// The per-repo [`InternerManager`] (Stage I — moved from per-table to
    /// per-repo ownership). Backed by the dedicated `"__interner__"` store,
    /// distinct from `"__tx__"` (recovery markers) so the two durability
    /// streams — chunked interner deltas vs. last-committed/next-tx markers —
    /// never collide. Built once on first access; every
    /// [`TableManager`](crate::table::TableManager) shares this one manager
    /// via `with_interner`, so a field name resolves to ONE id across all
    /// tables in the repo. V2 WAL recovery resolves this single interner
    /// instead of a per-table one (Stage I keystone).
    pub(crate) async fn repo_interner(&self) -> DbResult<InternerManager> {
        self.catalog
            .repo_interner
            .get_or_try_init(|| async {
                let store: Arc<dyn Store> = self.repo.store_get("__interner__").await?;
                Ok::<InternerManager, DbError>(InternerManager::new_labeled(
                    store,
                    self.name().to_string(),
                ))
            })
            .await
            .cloned()
    }

    /// D2 P1d-2b: produce a BACKGROUND clone for the drain task to own.
    /// Identical to a plain [`Clone`] except `live_weak` is dead (the task
    /// does not count toward repo liveness) and `worker_lifecycle = None`
    /// (the task does not retain foreground lifecycle ownership).
    /// See [`Drainer::spawn`](crate::tx::drainer::Drainer::spawn).
    fn clone_for_background(&self) -> Self {
        let mut bg = self.clone();
        bg.live_weak = Weak::new();
        // Background handles own only drainer resources, never foreground
        // lifecycle ownership.
        bg.worker_lifecycle = None;
        bg
    }

    /// D2 P1d-2b: the repo's background drainer, spawned once (single-owner).
    ///
    /// First call lazily creates the [`Drainer`](crate::tx::drainer::Drainer)
    /// and starts its background loop (a leak-free task owning a
    /// `clone_for_background` of this core plus a `Weak<()>` of the repo's
    /// liveness token). Subsequent calls (from any foreground clone) return
    /// the same `Arc<Drainer>` via the shared `OnceLock`. The commit path
    /// calls `repo.drainer().wake()` after publishing a version to nudge
    /// the loop.
    pub(crate) fn drainer(&self) -> Arc<crate::tx::drainer::Drainer> {
        #[cfg(test)]
        if self
            .background_drainer_disabled
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Arc::clone(
                self.horizon
                    .drainer_cell()
                    .get_or_init(|| Arc::new(crate::tx::drainer::Drainer::new())),
            );
        }

        Arc::clone(self.horizon.drainer_cell().get_or_init(|| {
            let drainer = Arc::new(crate::tx::drainer::Drainer::new());
            // 50 ms backstop: prompt drain even if a wake is missed, cheap
            // when idle (the loop short-circuits when nothing is undrained).
            const DRAIN_INTERVAL_MS: u64 = 50;
            // A background core asking for the drainer (shouldn't happen on
            // the commit path) carries a dead weak, so the loop exits at once.
            drainer.spawn(
                self.clone_for_background(),
                self.live_weak.clone(),
                std::time::Duration::from_millis(DRAIN_INTERVAL_MS),
            );
            drainer
        }))
    }
}
