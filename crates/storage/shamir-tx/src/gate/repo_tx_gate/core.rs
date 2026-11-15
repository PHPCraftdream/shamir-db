//! Gate construction, commit locks, version allocation, and publication.
use super::RepoTxGate;
use crate::completion_tracker::CompletionTracker;
use crate::version_guard::VersionGuard;
use shamir_collections::THasher;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

impl RepoTxGate {
    /// Create a new gate, optionally seeded from durable markers read
    /// on repo open.
    pub fn new(last_committed: u64, next_tx_id_seed: u64) -> Self {
        Self {
            commit_mutex: tokio::sync::Mutex::new(()),
            version_counter: AtomicU64::new(last_committed),
            last_committed_version: Arc::new(AtomicU64::new(last_committed)),
            receipt_native: AtomicBool::new(false),
            next_tx_id: AtomicU64::new(next_tx_id_seed.max(1)),
            active_snapshots: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            snapshot_versions: Arc::new(scc::TreeIndex::new()),
            active_serializable_count: Arc::new(AtomicU64::new(0)),
            active_snapshots_opening: AtomicU64::new(0),
            commit_write_log: scc::TreeIndex::new(),
            pending_commits: std::sync::Mutex::new(Vec::new()),
            completion: Arc::new(CompletionTracker::with_watermark(last_committed)),
            // P1d-1: seed the durable watermark at `last_committed` — on repo
            // open everything visible is, by induction, also durable in history
            // (visibility was driven by the inline materialize path). Under the
            // current inline path the durable watermark is kept in lock-step
            // with the visibility watermark by `mark_durable` on each commit.
            durable_completion: Arc::new(CompletionTracker::with_watermark(last_committed)),
            orphan_entries: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            wal_unresolved: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            requires_reopen: Arc::new(AtomicBool::new(false)),
            writes_fenced: Arc::new(AtomicBool::new(false)),
            orphaned_post_wal: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            orphan_max: Arc::new(AtomicU64::new(0)),
            active_post_wal: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            oldest_observation: arc_swap::ArcSwapOption::empty(),
            warning_epoch: tokio::time::Instant::now(),
            next_visibility_warning_ns: AtomicU64::new(0),
            #[cfg(test)]
            visibility_warning_count: AtomicU64::new(0),
            durable_progress: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Fresh gate for a new repo (no history).
    pub fn fresh() -> Self {
        Self::new(0, 1)
    }

    /// Current committed version visible to readers.
    pub fn last_committed(&self) -> u64 {
        self.last_committed_version.load(Ordering::Acquire)
    }

    /// Current version allocation high-water mark — the next version that
    /// will be allocated by [`assign_next_version_guarded`](Self::assign_next_version_guarded).
    ///
    /// This counter is incremented BEFORE any commit-phase writes happen
    /// (both tx-commit and non-tx paths), so a snapshot opened at version `V`
    /// can safely conclude "no writer has even STARTED a commit since I opened"
    /// when observing `version_allocation_high_water_mark() <= V`. This is
    /// strictly stronger than `last_committed()`, which only advances AFTER
    /// Phase 6 (post-5a/5c data+index writes) and thus has a false-negative
    /// window where concurrent writes are MVCC-visible but not yet published.
    ///
    /// Happens-before argument (the reader's half — see
    /// [`assign_next_version`](Self::assign_next_version) for the writer's
    /// half, which uses `AcqRel`, not `Relaxed`; a `Relaxed` store never
    /// synchronizes-with any load regardless of that load's own ordering,
    /// so both halves of the pair must be strengthened together):
    ///
    /// This `Acquire` load synchronizes-with the `AcqRel` `fetch_add` in
    /// `assign_next_version`. By definition of `synchronizes-with`, if this
    /// load observes a value `fetch_add` produced (i.e. `counter <= V`
    /// means no concurrent writer's `fetch_add` has run since this snapshot
    /// was taken), every memory operation SEQUENCED-BEFORE that concurrent
    /// writer's `fetch_add` on ITS thread — which includes nothing yet,
    /// since `fetch_add` runs at the very start of the commit pipeline,
    /// strictly before Phase 5a/5c's data/index writes — is guaranteed
    /// NOT yet visible to this reader either. Equivalently: any write this
    /// reader COULD observe from a concurrent committer implies that
    /// committer's `fetch_add` already happened-before this load (program
    /// order on the writer's thread + release sequence), which this
    /// `Acquire` load would then observe as `counter > V` — so `counter <=
    /// V` soundly rules out every such write.
    pub fn version_allocation_high_water_mark(&self) -> u64 {
        self.version_counter.load(Ordering::Acquire)
    }

    /// cancel-safe: yes — single `tokio::sync::Mutex::lock().await`,
    /// which is documented cancel-safe (`drop` of the future releases
    /// the wait without acquiring the lock).
    ///
    /// Lock the commit gate. Returns a tokio `MutexGuard`. After Stage B,
    /// the guard covers only the sequencer section: SSI validate → phantom
    /// validate → assign_version → WAL begin → materialize → publish →
    /// record_commit_writes. Phase 1 (interner merge) and Phase 2.5/2.6
    /// (uwl_guards + unique re-validation) run pre-lock. Phase 6.5/7 and
    /// HNSW promote run post-lock.
    pub async fn commit_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.commit_mutex.lock().await
    }

    /// Non-blocking attempt to acquire the commit gate.
    ///
    /// Used by the group-commit orchestrator: if `try_lock` succeeds the
    /// caller becomes the LEADER (processes its own tx + any followers
    /// queued in `pending_commits`). If it fails, the caller becomes a
    /// FOLLOWER and enqueues itself for the current leader to process.
    pub fn try_commit_lock(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.commit_mutex.try_lock().ok()
    }

    /// Allocate the next MVCC version. Called under `commit_lock`.
    ///
    /// `AcqRel` (not `Relaxed`, #1110 round 2): `version_allocation_high_water_mark`'s
    /// `Acquire` load must synchronize-with this RMW's store half for the
    /// staleness gate's happens-before argument to hold. A `Relaxed` store
    /// never participates in a synchronizes-with relationship with any
    /// `Acquire` load of the same atomic, regardless of that load's
    /// ordering — "Acquire on the reader" alone buys nothing without a
    /// matching `Release` (or `AcqRel`) on the writer. This was the exact
    /// gap in this task's own round-1 delivery: the doc comment there
    /// claimed the pairing was sound while the store stayed `Relaxed`.
    #[doc(hidden)] // External engine tests and tx_overhead bench use the bare allocator.
    pub fn assign_next_version(&self) -> u64 {
        self.version_counter.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Allocate the next MVCC version and return a RAII [`VersionGuard`]
    /// that owns the terminal-mark obligation.
    ///
    /// Pre-WAL Drop marks both trackers Aborted; commit marks visibility
    /// Materialized. After VersionGuard::wal_accepted, an armed Drop instead
    /// registers an orphan for the engine drainer to replay and complete.
    /// Visibility completion advances last_committed from the watermark.
    pub fn assign_next_version_guarded(&self) -> VersionGuard {
        let version = self.assign_next_version();
        VersionGuard::new(
            version,
            Arc::clone(&self.completion),
            Arc::clone(&self.durable_completion),
            Arc::clone(&self.last_committed_version),
            Arc::clone(&self.orphaned_post_wal),
            Arc::clone(&self.orphan_max),
            Arc::clone(&self.active_post_wal),
            Arc::clone(&self.orphan_entries),
            Arc::clone(&self.wal_unresolved),
            Arc::clone(&self.requires_reopen),
        )
    }

    /// Publish: monotonically advance `last_committed_version` atomically.
    ///
    /// Legacy plain-store fixture hook; does not settle completion. Production
    /// publishers use `raise_published`, and transaction snapshots read W.
    pub fn publish_committed(&self, version: u64) {
        self.raise_published(version);
    }

    /// Test fixture seam: plain-store `last_committed_version`, downward too, to
    /// synthesize a lagging published marker. Never use on a production path.
    #[doc(hidden)]
    pub fn set_published_for_fixture(&self, version: u64) {
        self.last_committed_version
            .store(version, Ordering::Release);
    }

    /// Test/bench seam: force-settle every version through `version`, then publish.
    /// Never use on a production write path: it closes pending and missing slots.
    #[doc(hidden)]
    pub fn publish_committed_max(&self, version: u64) {
        self.completion.settle_through(version);
        self.raise_published(version);
    }

    /// Eager, non-contiguous floor for latest-state reads, NOT a snapshot source.
    pub fn raise_published(&self, version: u64) {
        // Relaxed load is fine as the initial guess — the CAS will re-read
        // on conflict. We only need Release on a successful store so the
        // new value is visible to Acquire loads in `last_committed()`.
        let mut current = self.last_committed_version.load(Ordering::Relaxed);
        loop {
            if current >= version {
                break; // already at or past `version`
            }
            match self.last_committed_version.compare_exchange_weak(
                current,
                version,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Sync `last_committed_version` from the completion tracker's watermark.
    ///
    /// Called after `completion.mark(version, Materialized)` on the tx commit
    /// path. The watermark is monotonic, so this only ever advances the atomic.
    /// Uses `raise_published` (CAS loop) to coexist safely with non-tx
    /// writes that also advance the atomic independently.
    pub fn sync_last_committed_from_watermark(&self) {
        let wm = self.completion.watermark();
        self.raise_published(wm);
    }

    /// Number of currently-open Serializable snapshots.
    ///
    /// Non-tx write paths use this to skip the SSI footprint recording
    /// when there is no Serializable observer to detect a conflict.
    /// The load is `Relaxed`.
    ///
    /// # Why `Relaxed` is sufficient (audit concurrency §2 item 5)
    ///
    /// The earlier justification here ("the tx opened AFTER the write
    /// committed, so the write is visible as historical data") does NOT
    /// hold in general. Consider the interleaving: (1) writer reads
    /// `active_serializable_count == 0` (Relaxed); (2) opener increments
    /// `active_serializable_count`; (3) opener reads `last_committed` for
    /// its snapshot version; (4) writer assigns its commit version and
    /// publishes. This can place the opener's snapshot BEFORE the writer's
    /// commit even though the writer saw zero observers.
    ///
    /// The consequence is, however, BOUNDED: a non-tx write is a blind
    /// write (no read-validate), so its serial order relative to a
    /// snapshot that missed it is still valid — the snapshot simply sees
    /// the write as not-yet-happened, and the write's footprint is not
    /// needed for that snapshot's predicate validation (the snapshot
    /// cannot conflict with a write it cannot see). A future
    /// Dekker-style re-check (Acquire on the counter, Release on
    /// `last_committed`) would tighten this further but is not required
    /// for correctness of the serial order; it is tracked as a separate,
    /// riskier fix.
    pub fn active_serializable_count(&self) -> u64 {
        self.active_serializable_count.load(Ordering::Relaxed)
    }

    /// Peek at the `next_tx_id` counter (for durable snapshot).
    pub fn peek_next_tx_id(&self) -> u64 {
        self.next_tx_id.load(Ordering::Relaxed)
    }
}
