//! Per-repo transactional gate — serialises the commit phase.
//!
//! One `RepoTxGate` lives per repo. It owns:
//! - A `commit_mutex` that serialises the critical commit section
//!   (assign version → SSI validation → physical writes → publish).
//! - A monotonic `version_counter` (AtomicU64) for MVCC versioning.
//! - A durable `last_committed_version` (AtomicU64) that recovery
//!   reads on repo open to know which versions are visible.
//! - An `active_snapshots` set so GC knows the oldest open snapshot.
//! - A `next_tx_id` counter for allocating unique tx identifiers.
//!
//! Non-tx writes bypass the gate entirely — zero overhead on
//! non-transactional hot paths.

use bytes::Bytes;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_collections::THasher;

use crate::completion_tracker::CompletionTracker;
use crate::pending_commit::PendingCommit;
use crate::version_guard::VersionGuard;
use crate::TxId;

// ── Phase C: commit-write log structs ──────────────────────────────

/// PROPOSED (Phase C). One captured commit's index-posting footprint for
/// the predicate-conflict check. Built engine-side from `TxContext` state
/// already collected at commit time (see `build_footprint_from_tx`).
///
/// Wire-shape mirror of the doc section 5.2 sketch. NOT durable: regenerated only
/// while a Serializable tx is alive — same lifetime discipline as
/// `MvccStore::version_cache` (mvcc_store.rs:446) and `active_snapshots`.
#[derive(Debug, Clone, Default)]
pub struct TableWriteFootprint {
    /// Set iff the tx wrote ANYTHING (data/index/counter) on this table.
    /// Drives the coarse `PredicateDep::TableScan` check.
    pub touched: bool,
    /// `SetPosting.key`s the tx staged for this table (already collected
    /// at `tx.index_write_set` — tx_context.rs:72 ; SetPosting variant at
    /// index_write_op.rs:15). `Bytes` is refcounted: this clone is cheap.
    /// Sorted ascending so future `binary_search` optimisations are free.
    pub inserted_index_keys: Vec<Bytes>,
}

/// PROPOSED (Phase C). One committed tx's write footprint, stored in the
/// commit-write log for predicate-conflict checks.
#[derive(Debug, Clone)]
pub struct CommitWriteRecord {
    pub commit_version: u64,
    /// Keyed by table_token (engine's `table_token()`).
    pub per_table: HashMap<u64, TableWriteFootprint, THasher>,
}

impl CommitWriteRecord {
    pub fn is_empty(&self) -> bool {
        self.per_table.is_empty()
    }
}

/// Per-repo transactional synchronisation point.
pub struct RepoTxGate {
    /// Serialises the commit critical section. After Stage B, the lock
    /// covers only: SSI validate → phantom validate → assign_version →
    /// WAL begin → materialize (5a/5b/5c) → publish → record_commit_writes.
    /// Phase 1 (interner merge) and Phase 2.5/2.6 (uwl_guards + unique
    /// re-validation) run BEFORE the lock. Phase 6.5/7 (markers + WAL
    /// cleanup) and HNSW promote run AFTER the lock.
    /// `tokio::sync::Mutex` because the guard lives across `.await`.
    commit_mutex: tokio::sync::Mutex<()>,

    /// Monotonic MVCC version counter. Incremented on every commit.
    version_counter: AtomicU64,

    /// The highest version that has been fully published. Readers
    /// see only versions ≤ this value.
    ///
    /// `Arc` so a [`VersionGuard`] can hold a clone and advance it on
    /// `commit()` / `Drop` without a back-reference to the gate.
    last_committed_version: Arc<AtomicU64>,

    /// Monotonic tx-id allocator.
    next_tx_id: AtomicU64,

    /// Set of open snapshot versions. GC must not delete any version
    /// ≥ `min(active_snapshots)`.
    active_snapshots: Arc<scc::HashMap<u64, (), THasher>>,

    /// Count of currently-open Serializable snapshots. Incremented when
    /// a Serializable tx opens a snapshot; decremented when its
    /// `SnapshotGuard` drops. Non-tx writes check this atomically:
    /// if zero, no Serializable tx can observe the footprint, so the
    /// `record_nontx_ssi_footprint` path is skipped entirely.
    active_serializable_count: Arc<AtomicU64>,

    /// PROPOSED (Phase C). Ring of recently-committed write footprints.
    /// Keyed by `commit_version` (monotonic), so a `range(snapshot..]`
    /// scan visits exactly the tx's conflict window. `scc::TreeIndex`
    /// is lock-free CAS-based — no `RwLock` poisoning and no contention
    /// on the gate's `commit_lock`. Empty unless at least one
    /// Serializable tx has committed.
    commit_write_log: scc::TreeIndex<u64, Arc<CommitWriteRecord>>,

    /// Stage D scaffolding: queue of transactions awaiting group-commit.
    /// Short-section `std::sync::Mutex` — only push/drain, no `.await`
    /// held across. The leader pops the entire vec under lock.
    pending_commits: std::sync::Mutex<Vec<PendingCommit>>,

    /// Tracks materialized/aborted state per version and maintains a
    /// contiguous watermark. The watermark drives `last_committed_version`
    /// via [`sync_last_committed_from_watermark`](Self::sync_last_committed_from_watermark).
    ///
    /// `Arc` so a [`VersionGuard`] can hold a clone and own the terminal
    /// `mark(version, …)` obligation by RAII without a back-reference to
    /// the gate.
    completion: Arc<CompletionTracker>,

    /// P1d-1: second `CompletionTracker` whose contiguous watermark tracks
    /// the highest version `V` for which **the value is durable in the history
    /// log** (i.e. Phase 5a / inline history write succeeded). Under inline
    /// materialize this watermark coincides with `last_committed_version`
    /// (visibility) because durable-history landing and ack-publish happen on
    /// the same code path; P1d-2 then decouples them by moving `history.transact`
    /// off the ack-path into a background drain.
    ///
    /// `State::Materialized` is reused here to mean "data for this version is
    /// durable in history". Nothing in the production read or commit path
    /// consumes this watermark in P1d-1 — only tests assert the invariant
    /// `durable_watermark() <= last_committed()` and the equality under inline
    /// materialize. P1d-2 will gate Phase-7 WAL truncation on it, and overlay
    /// GC will use it as the lower drain bound.
    durable_completion: Arc<CompletionTracker>,
}

/// RAII guard that removes a snapshot from `active_snapshots` on drop.
/// If the snapshot was opened for a Serializable transaction, also
/// decrements `active_serializable_count` so non-tx writes stop paying
/// the SSI-footprint overhead once no Serializable tx is watching.
pub struct SnapshotGuard {
    version: u64,
    snapshots: Arc<scc::HashMap<u64, (), THasher>>,
    /// Non-None iff this guard was opened for a Serializable tx.
    /// Holds the shared counter so Drop can decrement it without
    /// holding a reference back to the `RepoTxGate`.
    serializable_count: Option<Arc<AtomicU64>>,
}

impl SnapshotGuard {
    /// The snapshot version this guard protects.
    pub fn version(&self) -> u64 {
        self.version
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        let _ = self.snapshots.remove(&self.version);
        if let Some(cnt) = &self.serializable_count {
            cnt.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl RepoTxGate {
    /// Create a new gate, optionally seeded from durable markers read
    /// on repo open.
    pub fn new(last_committed: u64, next_tx_id_seed: u64) -> Self {
        Self {
            commit_mutex: tokio::sync::Mutex::new(()),
            version_counter: AtomicU64::new(last_committed),
            last_committed_version: Arc::new(AtomicU64::new(last_committed)),
            next_tx_id: AtomicU64::new(next_tx_id_seed),
            active_snapshots: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            active_serializable_count: Arc::new(AtomicU64::new(0)),
            commit_write_log: scc::TreeIndex::new(),
            pending_commits: std::sync::Mutex::new(Vec::new()),
            completion: Arc::new(CompletionTracker::with_watermark(last_committed)),
            // P1d-1: seed the durable watermark at `last_committed` — on repo
            // open everything visible is, by induction, also durable in history
            // (visibility was driven by the inline materialize path). Under the
            // current inline path the durable watermark is kept in lock-step
            // with the visibility watermark by `mark_durable` on each commit.
            durable_completion: Arc::new(CompletionTracker::with_watermark(last_committed)),
        }
    }

    /// Fresh gate for a new repo (no history).
    pub fn fresh() -> Self {
        Self::new(0, 1)
    }

    /// Allocate a unique tx-id. Lock-free.
    pub fn fresh_tx_id(&self) -> TxId {
        TxId::new(self.next_tx_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Current committed version visible to readers.
    pub fn last_committed(&self) -> u64 {
        self.last_committed_version.load(Ordering::Acquire)
    }

    /// cancel-safe: yes — single `scc::HashMap::insert_async` is CAS-
    /// based and either completes or leaves the map unchanged on
    /// cancellation. If the future is dropped after insertion but before
    /// returning the guard, the snapshot entry leaks (no Drop runs);
    /// since callers always await this to completion or never call it,
    /// in practice this is cancel-safe.
    ///
    /// Register a snapshot at the current `last_committed` version and
    /// return a RAII guard. On drop the snapshot is removed.
    /// Use [`open_snapshot_serializable`](Self::open_snapshot_serializable)
    /// when the transaction's isolation level is `Serializable`.
    pub async fn open_snapshot(&self) -> SnapshotGuard {
        let version = self.last_committed();
        let _ = self.active_snapshots.insert_async(version, ()).await;
        SnapshotGuard {
            version,
            snapshots: Arc::clone(&self.active_snapshots),
            serializable_count: None,
        }
    }

    /// Like [`open_snapshot`](Self::open_snapshot) but also increments
    /// `active_serializable_count` for the lifetime of the guard.
    ///
    /// Must be called instead of `open_snapshot` when opening a snapshot
    /// for a `Serializable` transaction so that non-tx writes know at
    /// least one Serializable observer is alive and must record their
    /// SSI footprint.
    ///
    /// cancel-safe: same guarantee as `open_snapshot`.
    pub async fn open_snapshot_serializable(&self) -> SnapshotGuard {
        let version = self.last_committed();
        let _ = self.active_snapshots.insert_async(version, ()).await;
        // Increment AFTER the snapshot version is registered so that
        // any concurrent non-tx write that races with this path already
        // sees the snapshot version in `active_snapshots` (GC safety).
        // The increment is AcqRel so it is visible to the non-tx write
        // that does a Relaxed load — a conservative over-approximation
        // is fine (the footprint is cheap to write).
        self.active_serializable_count
            .fetch_add(1, Ordering::AcqRel);
        SnapshotGuard {
            version,
            snapshots: Arc::clone(&self.active_snapshots),
            serializable_count: Some(Arc::clone(&self.active_serializable_count)),
        }
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
    pub fn assign_next_version(&self) -> u64 {
        self.version_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Allocate the next MVCC version and return a RAII [`VersionGuard`]
    /// that owns the terminal-mark obligation.
    ///
    /// The guard's `Drop` marks the version `Aborted` (advancing the
    /// watermark past it) unless [`VersionGuard::commit`] was called first,
    /// which marks it `Materialized`. Both paths advance
    /// `last_committed_version` from the resulting watermark — identical to
    /// the manual `mark` + [`sync_last_committed_from_watermark`] pair this
    /// replaces. The compiler thus enforces that every allocated version is
    /// terminally marked exactly once.
    pub fn assign_next_version_guarded(&self) -> VersionGuard {
        let version = self.assign_next_version();
        VersionGuard::new(
            version,
            Arc::clone(&self.completion),
            Arc::clone(&self.durable_completion),
            Arc::clone(&self.last_committed_version),
        )
    }

    /// Publish: update `last_committed_version` atomically.
    ///
    /// Must be called under `commit_lock` on the tx commit path, where
    /// monotonic ordering is guaranteed by the lock. Both tx commits and
    /// non-tx writes now advance `last_committed` via
    /// [`publish_committed_max`](Self::publish_committed_max) (the monotonic
    /// fetch_max CAS), so every committed version becomes visible to
    /// subsequently-opened snapshots. Prefer `publish_committed_max` for any
    /// new publish site; this plain store is retained for the
    /// `commit_lock`-guarded tx path where strict monotonicity already holds.
    pub fn publish_committed(&self, version: u64) {
        self.last_committed_version
            .store(version, Ordering::Release);
    }

    /// Publish: advance `last_committed_version` to `version` if it is
    /// currently lower, using an atomic compare-and-swap loop.
    ///
    /// Safe to call without `commit_lock` because it only moves the counter
    /// forwards — it never moves it backwards even if concurrent tx commits
    /// or other non-tx writes race with this call. Both the non-tx write path
    /// (`MvccStore::set_versioned` / `set_versioned_many` /
    /// `delete_versioned`) and the tx commit path call this, so every
    /// committed version advances the reader-visible floor and becomes visible
    /// to snapshots/txs opened afterwards.
    pub fn publish_committed_max(&self, version: u64) {
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
    /// Uses `publish_committed_max` (CAS loop) to coexist safely with non-tx
    /// writes that also advance the atomic independently.
    pub fn sync_last_committed_from_watermark(&self) {
        let wm = self.completion.watermark();
        self.publish_committed_max(wm);
    }

    /// Minimum alive snapshot version — for GC. Returns
    /// `last_committed()` if no snapshots are open.
    pub fn min_alive(&self) -> u64 {
        let mut min = u64::MAX;
        self.active_snapshots.scan(|k, _| {
            if *k < min {
                min = *k;
            }
        });
        if min == u64::MAX {
            self.last_committed()
        } else {
            min
        }
    }

    /// True if no transaction has an open snapshot.
    pub fn active_snapshots_empty(&self) -> bool {
        self.active_snapshots.is_empty()
    }

    /// Number of currently-open Serializable snapshots.
    ///
    /// Non-tx write paths use this to skip the SSI footprint recording
    /// when there is no Serializable observer to detect a conflict.
    /// The load is `Relaxed` — a stale read of zero means a non-tx write
    /// may skip a footprint that a just-opening Serializable tx has not
    /// yet registered.  That window is harmless: the tx opened AFTER the
    /// non-tx write committed, so the write was already committed at the
    /// tx's snapshot and will be visible as historical data, not as a
    /// phantom in the predicate window.
    pub fn active_serializable_count(&self) -> u64 {
        self.active_serializable_count.load(Ordering::Relaxed)
    }

    /// Peek at the `next_tx_id` counter (for durable snapshot).
    pub fn peek_next_tx_id(&self) -> u64 {
        self.next_tx_id.load(Ordering::Relaxed)
    }

    // ── Stage D: group-commit queue ───────────────────────────────────

    /// Enqueue a `PendingCommit` for the next group-commit batch.
    pub fn enqueue_pending(&self, p: PendingCommit) {
        self.pending_commits.lock().unwrap().push(p);
    }

    /// Drain all pending commits, returning them to the leader.
    pub fn drain_pending(&self) -> Vec<PendingCommit> {
        let mut guard = self.pending_commits.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    // ── Phase C: commit-write log ────────────────────────────────────

    /// PROPOSED (Phase C). Append one footprint at publish time.
    ///
    /// Called by `commit_tx::materialize` UNDER `commit_lock`, immediately
    /// after `gate.publish_committed(commit_version)`. The caller already
    /// holds `commit_lock`, so the append is serialised against every other
    /// publish. We use `TreeIndex::insert` (CAS-based, lock-free).
    ///
    /// Zero-overhead invariant: NO-OP off Serializable AND no-op when the
    /// footprint has no touched tables. The caller MUST construct the
    /// footprint via `build_footprint_from_tx` which short-circuits to
    /// empty under Snapshot — but we re-gate here defensively, so this
    /// function is safe to call unconditionally.
    pub fn record_commit_writes(&self, rec: CommitWriteRecord) {
        if rec.per_table.is_empty() {
            return;
        }
        let _ = self
            .commit_write_log
            .insert(rec.commit_version, Arc::new(rec));
    }

    /// PROPOSED (Phase C). True iff some committer in the window
    /// `(snapshot, last_committed()]` wrote a footprint that conflicts
    /// with `dep`. Called by `pre_commit` Phase 2-bis UNDER `commit_lock`.
    ///
    /// Walks ONLY records with `snapshot < commit_version <= last_committed`.
    /// The upper bound `last_committed` is important for non-tx footprints:
    /// non-tx writes append to the log via `record_nontx_ssi_footprint` at
    /// the version assigned by `set_versioned` / `delete_versioned`, but they
    /// do NOT call `publish_committed`. So their version may be higher than
    /// `last_committed`. Such a record is "pending" and must NOT be treated as
    /// a concurrent committed write relative to this tx's predicate window.
    ///
    /// Uses `(snapshot + 1)..=last_committed` to express the exclusive lower
    /// / inclusive upper bound (snapshot is always far from `u64::MAX` in
    /// practice — versions are monotonic from a small seed).
    pub fn predicate_conflicts(
        &self,
        dep: &crate::predicate_set::PredicateDep,
        snapshot: u64,
    ) -> bool {
        let guard = scc::ebr::Guard::new();
        let last = self.last_committed_version.load(Ordering::Acquire);
        if last <= snapshot {
            // Fast path: no new commits since snapshot.
            return false;
        }
        // `snapshot + 1` is safe: version 0 is the initial seed, and
        // a tx at snapshot u64::MAX cannot exist (versions are monotonic
        // from a small seed and would exhaust before reaching MAX).
        let start = snapshot.saturating_add(1);
        for (_v, rec) in self.commit_write_log.range(start..=last, &guard) {
            if record_conflicts(rec, dep) {
                return true;
            }
        }
        false
    }

    /// PROPOSED (Phase C). Drop records with `commit_version <= floor`.
    ///
    /// Called by the same GC tick that prunes `MvccStore::version_cache`.
    /// The caller MUST pass `gate.min_alive()` — never anything higher.
    pub fn prune_commit_log_below(&self, floor: u64) -> usize {
        // Use `remove_range` for efficiency.
        let guard = scc::ebr::Guard::new();
        let count = self.commit_write_log.range(..=floor, &guard).count();
        self.commit_write_log.remove_range(..=floor);
        count
    }

    /// Number of records currently in the commit-write log.
    /// Useful for telemetry and engine-side tests (GC prune assertions).
    /// Not on any hot path — walks the tree under an EBR guard.
    pub fn commit_log_len(&self) -> usize {
        let guard = scc::ebr::Guard::new();
        self.commit_write_log.range::<u64, _>(.., &guard).count()
    }

    /// Access the completion tracker (P1a scaffolding; abort-path wired in P1b).
    pub fn completion(&self) -> &CompletionTracker {
        &self.completion
    }

    // ── P1d-1: durable watermark (additive, zero behavior change) ──────────

    /// Highest contiguous version V for which the value is durable in the
    /// history log.
    ///
    /// Under the current inline-materialize path this equals
    /// [`last_committed`](Self::last_committed) by construction (every commit
    /// calls [`mark_durable`](Self::mark_durable) at the same point it commits
    /// its [`VersionGuard`]). P1d-2 will decouple the two by moving
    /// `history.transact` of the tx-path off the ack-path into a background
    /// drain leader; until then the durable watermark is a redundant view
    /// kept current so the rest of the machinery can be wired and tested.
    ///
    /// Invariant: `durable_watermark() <= last_committed()` always — every
    /// site that marks durable does so AFTER the visibility mark
    /// (`guard.commit()`), so visibility either equals or leads durable, never
    /// trails it.
    pub fn durable_watermark(&self) -> u64 {
        self.durable_completion.watermark()
    }

    /// Record that `version`'s value is durable in the history log.
    ///
    /// `State::Materialized` is reused as the "durable" terminal state for
    /// this tracker: there is no Aborted analogue (an aborted version was
    /// never written and is irrelevant to durability — but the contiguous
    /// watermark still needs to advance past it). For now we rely on the
    /// non-tx path and tx-path Complete-outcome callers to mark every
    /// non-aborted version. Aborted versions are marked here too (see
    /// [`mark_durable_aborted`](Self::mark_durable_aborted)) so the
    /// contiguous prefix on the durable tracker matches the visibility
    /// tracker under inline materialize.
    ///
    /// Idempotent: marking the same version twice (or marking a version at
    /// or below the current watermark) is a no-op.
    pub fn mark_durable(&self, version: u64) {
        self.durable_completion
            .mark(version, crate::completion_tracker::State::Materialized);
    }

    /// Record that `version` will never be durable (aborted before any
    /// physical write), so the contiguous durable watermark advances past
    /// it. Used by the same RAII path that marks the visibility tracker
    /// `Aborted` on early returns; under inline materialize this keeps
    /// `durable_watermark() == last_committed()` even when an SSI / phantom
    /// / WAL-begin abort burns a version.
    pub fn mark_durable_aborted(&self, version: u64) {
        self.durable_completion
            .mark(version, crate::completion_tracker::State::Aborted);
    }
}

/// PROPOSED (Phase C). Conflict-check for ONE committed record.
/// Free function so it stays testable without a `RepoTxGate` instance.
/// Public so the group-commit leader can check batch-local footprints
/// against a survivor's predicates (inter-batch phantom detection, P3a).
pub fn record_conflicts(rec: &CommitWriteRecord, dep: &crate::predicate_set::PredicateDep) -> bool {
    match dep {
        crate::predicate_set::PredicateDep::TableScan { table_token } => {
            rec.per_table.get(table_token).is_some_and(|f| f.touched)
        }
        crate::predicate_set::PredicateDep::IndexRange {
            table_token,
            index_id,
            lo,
            hi,
        } => rec.per_table.get(table_token).is_some_and(|f| {
            f.inserted_index_keys
                .iter()
                .any(|k| crate::predicate_set::key_in_interval(k, *index_id, lo, hi))
        }),
    }
}

/// PROPOSED (Phase C). Project the on-commit `TxContext` state into a
/// `CommitWriteRecord`. Zero-cost off Serializable: returns an empty
/// record whose `per_table` map is empty (no allocations except the
/// stack-resident empty `HashMap`), and the caller's `record_commit_writes`
/// no-ops on it.
///
/// Called by `commit_tx::materialize` AT publish time.
/// Inputs already collected by the existing pipeline:
///   - `tx.index_write_set` (tx_context.rs:72) — SetPosting keys per table.
///   - `tx.write_set`       (tx_context.rs:67) — touched tables (data).
///   - `tx.counter_deltas`  (tx_context.rs:93) — touched tables (counter).
pub fn build_footprint_from_tx(tx: &crate::TxContext, commit_version: u64) -> CommitWriteRecord {
    let mut rec = CommitWriteRecord {
        commit_version,
        per_table: HashMap::with_hasher(THasher::default()),
    };
    if tx.isolation != crate::IsolationLevel::Serializable {
        // Zero-overhead: Snapshot/non-tx publishes nothing.
        return rec;
    }

    // Touched-bit: any table that ANY non-empty staging touched.
    for token in tx.write_set.keys() {
        rec.per_table.entry(*token).or_default().touched = true;
    }
    for token in tx.counter_deltas.keys() {
        rec.per_table.entry(*token).or_default().touched = true;
    }

    // Precise index-posting keys (only SetPosting; RemovePosting does not
    // introduce a phantom in a predicate — coarse `touched` covers it).
    for (token, op) in &tx.index_write_set {
        // Index ops always touch their table even if SetPosting/Remove split.
        rec.per_table.entry(*token).or_default().touched = true;
        if let crate::IndexWriteOp::SetPosting { key, .. } = op {
            rec.per_table
                .entry(*token)
                .or_default()
                .inserted_index_keys
                .push(key.clone());
        }
    }

    // Sort each table's keys ascending — frees a future binary_search
    // optimisation (doc section 5.4) and makes test assertions stable.
    for f in rec.per_table.values_mut() {
        f.inserted_index_keys.sort_unstable();
    }

    rec
}
