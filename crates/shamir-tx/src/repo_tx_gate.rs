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
use shamir_collections::{TFxMap, THasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    pub per_table: TFxMap<u64, TableWriteFootprint>,
}

impl CommitWriteRecord {
    pub fn is_empty(&self) -> bool {
        self.per_table.is_empty()
    }
}

/// Per-repo transactional synchronisation point.
pub struct RepoTxGate {
    /// Serialises the commit critical section. NOT every commit takes
    /// this mutex — only the paths that need to serialize the
    /// validate→publish window:
    ///   - The legacy **AsyncIndex** path (`commit_tx_inner_legacy_async`)
    ///     holds it across the full SSI validate → phantom validate →
    ///     assign_version → WAL begin → materialize → publish →
    ///     record_commit_writes section.
    ///   - The lock-free `commit_tx_lockfree` path takes it ONLY for
    ///     Serializable txs, for the narrower validate→publish window
    ///     (CRIT-4 / #438: prevents write-skew between two Serializable
    ///     txs with disjoint write-sets). Snapshot txs on the same
    ///     lock-free path do NOT take it — they keep full lock-free
    ///     parallelism (no SSI validation, no predicate check).
    ///
    /// Phase 1 (interner merge) and Phase 2.5/2.6 (uwl_guards + unique
    /// re-validation) run BEFORE the lock on every path. Phase 6.5/7
    /// (markers + WAL cleanup) and HNSW promote run AFTER the lock.
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

    /// Refcounted set of open snapshot versions. GC must not delete any
    /// version >= `min(active_snapshots)`. The value is a refcount so that
    /// multiple concurrent snapshots pinned to the SAME version do not
    /// corrupt each other's registration (insert bumps, drop decrements,
    /// the entry is removed only when the count reaches zero).
    active_snapshots: Arc<scc::HashMap<u64, u64, THasher>>,

    /// Count of currently-open Serializable snapshots. Incremented when
    /// a Serializable tx opens a snapshot; decremented when its
    /// `SnapshotGuard` drops. Non-tx writes check this atomically:
    /// if zero, no Serializable tx can observe the footprint, so the
    /// `record_nontx_ssi_footprint` path is skipped entirely.
    active_serializable_count: Arc<AtomicU64>,

    /// A10 in-flight barrier: count of `open_snapshot` /
    /// `open_snapshot_serializable` calls that have incremented this counter
    /// (BEFORE reading `last_committed()`) but have not yet completed their
    /// registration in `active_snapshots`. While this is non-zero, vacuum's
    /// L6 fast path MUST NOT physically delete any version — a reader is
    /// mid-registration and its chosen floor version is not yet visible in
    /// `active_snapshots`. This closes the TOCTOU for an UNBOUNDED number
    /// of writer cycles: no matter how many writes happen while a reader is
    /// stalled, vacuum defers all deletion until every in-flight opener has
    /// completed registration.
    ///
    /// Ordering: the increment uses `AcqRel` so it is visible to vacuum's
    /// `Acquire` load BEFORE the reader's subsequent `last_committed()` read
    /// (which is itself an `Acquire` load on a different atomic — the
    /// `AcqRel` store-release on the counter establishes a happens-before
    /// edge). The decrement also uses `AcqRel` so it is visible to vacuum
    /// only AFTER the registration (`entry_async` insert) has completed.
    active_snapshots_opening: AtomicU64,

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

    /// D2 P1e — durable-progress signal. Notified by
    /// [`mark_durable`](Self::mark_durable) every time a version is marked
    /// durable (the drainer's `drain_step` calls it once per drained version).
    /// Backpressured committers park on [`durable_notified`](Self::durable_notified)
    /// and re-check the `last_committed() - durable_watermark()` gap when woken,
    /// so they yield latency ONLY under sustained write pressure and resume the
    /// instant the drain makes progress. Lock-free / wait-free to signal.
    durable_progress: Arc<tokio::sync::Notify>,
}

/// RAII guard that removes a snapshot from `active_snapshots` on drop.
/// If the snapshot was opened for a Serializable transaction, also
/// decrements `active_serializable_count` so non-tx writes stop paying
/// the SSI-footprint overhead once no Serializable tx is watching.
pub struct SnapshotGuard {
    /// The versions this guard has registered in `active_snapshots`,
    /// paired with whether the registration used a refcount bump
    /// (always true under the refcount scheme). On drop, each entry's
    /// refcount is decremented, and the key is removed when it hits
    /// zero. A guard may carry MORE than one version when the floor
    /// moved during `open_snapshot` — the stale registration is cleaned
    /// up here rather than mid-open so there is never a zero-registration
    /// window for a concurrently-racing vacuum.
    registered: Vec<u64>,
    snapshots: Arc<scc::HashMap<u64, u64, THasher>>,
    /// Non-None iff this guard was opened for a Serializable tx.
    /// Holds the shared counter so Drop can decrement it without
    /// holding a reference back to the `RepoTxGate`.
    serializable_count: Option<Arc<AtomicU64>>,
}

impl SnapshotGuard {
    /// The snapshot version this guard protects — the FINAL (highest)
    /// registered version, which is the version the reader reads at.
    pub fn version(&self) -> u64 {
        *self.registered.last().unwrap_or(&0)
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        // Decrement the refcount for every registered version; remove
        // the entry when the count reaches zero. Each remove runs under
        // the per-entry exclusive lock (scc entry API), so the read-
        // modify-write is race-free.
        for &v in &self.registered {
            // Use the sync `entry` API — Drop is not async. `entry` takes
            // the per-entry exclusive lock, so we can read-and-decrement
            // atomically.
            match self.snapshots.entry(v) {
                scc::hash_map::Entry::Occupied(mut e) => {
                    let count = e.get_mut();
                    if *count <= 1 {
                        let _ = e.remove_entry();
                    } else {
                        *count -= 1;
                    }
                }
                scc::hash_map::Entry::Vacant(_) => {
                    // Already removed by a prior drop or never inserted —
                    // safe to ignore (idempotent).
                }
            }
        }
        if let Some(cnt) = &self.serializable_count {
            cnt.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// A10 in-flight barrier RAII guard. Increments the counter on creation,
/// decrements on drop (including cancellation). Ensures vacuum's fast path
/// never deletes a version while a reader is between its floor-read and
/// registration completion.
pub(crate) struct OpeningBarrier<'a> {
    counter: &'a AtomicU64,
}

impl<'a> OpeningBarrier<'a> {
    fn new(counter: &'a AtomicU64) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl<'a> Drop for OpeningBarrier<'a> {
    fn drop(&mut self) {
        // AcqRel: the decrement must be visible to vacuum's Acquire load
        // ONLY AFTER the registration that preceded this drop is complete.
        // fetch_sub returns the previous value; we only assert it was > 0
        // in debug builds to catch double-decrement bugs.
        let prev = self.counter.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "OpeningBarrier double-decrement");
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
            durable_progress: Arc::new(tokio::sync::Notify::new()),
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

    /// cancel-safe: yes — the `entry_async` calls are CAS-based and either
    /// complete or leave the map unchanged on cancellation. If the future is
    /// dropped after a registration but before returning the guard, the
    /// snapshot entry leaks (no Drop runs); since callers always await this
    /// to completion or never call it, in practice this is cancel-safe.
    ///
    /// Register a snapshot at the current `last_committed` version and
    /// return a RAII guard. On drop the snapshot is removed (refcount-
    /// decremented).
    ///
    /// TOCTOU fix (A10): registration happens BEFORE the snapshot version is
    /// finalised, then the floor is re-checked. If `last_committed` advanced
    /// between the initial read and the registration completing, a SECOND
    /// registration at the new floor is inserted BEFORE the stale one is
    /// removed, so there is never a zero-registration window during which a
    /// racing vacuum could observe `active_snapshots_empty() == true` and
    /// delete a version this reader is about to pin. The guard carries ALL
    /// registered versions and cleans them up on drop.
    ///
    /// Use [`open_snapshot_serializable`](Self::open_snapshot_serializable)
    /// when the transaction's isolation level is `Serializable`.
    pub async fn open_snapshot(&self) -> SnapshotGuard {
        let registered = self.register_snapshot().await;
        // The snapshot version the reader reads at is the FINAL (highest)
        // registered version — the floor can only move forward.
        SnapshotGuard {
            registered,
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
        let registered = self.register_snapshot().await;
        // Increment AFTER the snapshot version is registered so that
        // any concurrent non-tx write that races with this path already
        // sees the snapshot version in `active_snapshots` (GC safety).
        // The increment is AcqRel so it is visible to the non-tx write
        // that does a Relaxed load — a conservative over-approximation
        // is fine (the footprint is cheap to write).
        self.active_serializable_count
            .fetch_add(1, Ordering::AcqRel);
        SnapshotGuard {
            registered,
            snapshots: Arc::clone(&self.active_snapshots),
            serializable_count: Some(Arc::clone(&self.active_serializable_count)),
        }
    }

    /// A10 TOCTOU fix: register-then-verify-then-reconcile, protected by
    /// an in-flight barrier.
    ///
    /// The in-flight barrier (`active_snapshots_opening`) is incremented
    /// BEFORE the first `last_committed()` read and decremented AFTER the
    /// final registration completes. While the counter is non-zero, vacuum's
    /// fast path defers ALL physical deletion — closing the race for an
    /// unbounded number of writer cycles (a stalled reader's floor version
    /// is never deleted, no matter how many writes happen while it is
    /// mid-registration).
    ///
    /// The barrier uses an RAII guard (`OpeningBarrier`) so the counter is
    /// correctly decremented even if the future is dropped/cancelled mid-
    /// registration — a leaked increment would permanently block vacuum's
    /// fast path.
    ///
    /// After registration:
    /// 1. Read the floor `v0 = last_committed()`.
    /// 2. Atomically bump the refcount for `v0` in `active_snapshots`.
    /// 3. Re-read the floor. If it advanced to `v1 > v0`, register `v1`
    ///    too (the stale `v0` entry is left for the guard's Drop to clean
    ///    up via refcount-decrement).
    ///
    /// Bounded retry: the floor only moves forward, so at most one
    /// reconciliation iteration is needed in practice.
    async fn register_snapshot(&self) -> Vec<u64> {
        // A10 barrier: increment BEFORE reading the floor. The AcqRel
        // store ensures vacuum's Acquire load sees the increment before
        // the reader's subsequent Acquire load of `last_committed()`.
        let _barrier = OpeningBarrier::new(&self.active_snapshots_opening);

        const MAX_RETRIES: usize = 4;
        let mut registered = Vec::with_capacity(2);
        let mut current = self.last_committed();
        loop {
            self.bump_refcount(current).await;
            registered.push(current);
            let next = self.last_committed();
            if next == current || registered.len() >= MAX_RETRIES {
                break;
            }
            // Floor moved forward — loop to register the new floor too.
            current = next;
        }
        registered
        // `_barrier` drops here: AcqRel decrement, visible to vacuum only
        // AFTER all registrations above are complete.
    }

    /// Atomically insert-or-bump the refcount for `version` in
    /// `active_snapshots`. Uses the per-entry exclusive lock held by
    /// `entry_async` so the read-modify-write is race-free against
    /// concurrent openers and droppers.
    async fn bump_refcount(&self, version: u64) {
        match self.active_snapshots.entry_async(version).await {
            scc::hash_map::Entry::Occupied(mut e) => {
                *e.get_mut() = e.get().saturating_add(1);
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(1u64);
            }
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
    ///
    /// A10 in-flight barrier awareness: while any `open_snapshot` call is
    /// mid-registration (`active_snapshots_opening > 0`), this function
    /// returns `0` — the maximally conservative floor. This means:
    /// * History GC (`gc_below`, `vacuum_key`, `purge_below_ts`): no version
    ///   is reclaimable (all versions `>= 0` are sacred).
    /// * `prune_commit_log_below`: no records are pruned (real commit
    ///   versions start at 1, so `commit_version <= 0` matches nothing).
    /// * `prune_version_cache`: no entries are evicted (all `version >= 0`).
    ///
    /// This protects the in-flight reader unconditionally: its floor is some
    /// `v_old <= last_committed()` (captured before the barrier increment),
    /// and since `min_alive() == 0 < v_old`, every version the reader might
    /// need is sacred. Once the reader completes registration, its version
    /// appears in `active_snapshots` and `min_alive()` returns the true
    /// minimum — GC resumes normally.
    pub fn min_alive(&self) -> u64 {
        // Fast path: no in-flight openers. This is the overwhelmingly common
        // case, so we check it FIRST to avoid the active_snapshots scan when
        // there is no contention.
        if self.active_snapshots_opening.load(Ordering::Acquire) > 0 {
            return 0;
        }
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
    ///
    /// A10 in-flight barrier awareness: returns `false` (i.e. "snapshots
    /// ARE active") while any `open_snapshot` call is mid-registration,
    /// even if the registration hasn't landed in `active_snapshots` yet.
    /// This ensures callers that use `active_snapshots_empty()` to decide
    /// whether to keep an anchor (e.g. `vacuum_key`'s scan path at
    /// `have_live_snapshot = !active_snapshots_empty()`) behave
    /// conservatively during the registration window.
    pub fn active_snapshots_empty(&self) -> bool {
        self.active_snapshots.is_empty() && !self.snapshots_opening()
    }

    /// A10 in-flight barrier: true if at least one `open_snapshot` call has
    /// begun (incremented the counter) but not yet completed registration in
    /// `active_snapshots`. Vacuum's fast path checks this to defer ALL
    /// physical deletion while a reader is mid-registration.
    pub fn snapshots_opening(&self) -> bool {
        self.active_snapshots_opening.load(Ordering::Acquire) > 0
    }

    /// Test-only: simulate a stalled `open_snapshot` caller that has
    /// incremented the in-flight barrier but not yet completed registration.
    /// Returns an RAII guard that decrements on drop. This lets tests
    /// deterministically reproduce the multi-generation stall scenario
    /// (reader captured floor, held in-flight across N writes, then
    /// completes) without relying on scheduler timing.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub(crate) fn test_hold_opening_barrier(&self) -> OpeningBarrier<'_> {
        OpeningBarrier::new(&self.active_snapshots_opening)
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

    /// Phase C — inverted batch predicate validation.
    ///
    /// Walks the commit window `(snapshot, last_committed()]` **once** and
    /// tests ALL `deps` against each `CommitWriteRecord`, short-circuiting
    /// on the FIRST conflict. This is the order-inverted equivalent of
    /// calling [`predicate_conflicts`](Self::predicate_conflicts) per dep:
    /// the set of conflicts detected is identical (a conflict exists iff
    /// *some* dep conflicts with *some* record in the window), but the cost
    /// drops from O(P×W) to O(W×P_avg) with a single shared EBR guard
    /// instead of P separate `Guard::new()` allocations.
    ///
    /// Returns the index of the FIRST conflicting dep (in `deps` iteration
    /// order), or `None` when no dep conflicts with any record in the window.
    /// The caller formats the dep for the `PhantomConflict` error.
    ///
    /// # Calling contract
    ///
    /// Both call sites (`pre_commit_locked_validate` in the legacy
    /// AsyncIndex path AND `pre_commit_locked` / `commit_tx_lockfree`)
    /// invoke this ONLY for Serializable txs (the caller gates on
    /// `tx.isolation == Serializable`). For Serializable txs the
    /// `commit_tx_lockfree` path takes `gate.commit_lock()` for the
    /// validate→publish window (CRIT-4 / #438: Serializable txs must
    /// serialize that window to prevent write-skew), so in practice
    /// this method runs UNDER `commit_lock` on every live call path.
    /// The lock-free claim that "the lock-free commit path does NOT
    /// hold the lock here" was true before CRIT-4 but is no longer
    /// accurate for the Serializable branch. Snapshot txs never reach
    /// this method (no predicate validation), so the no-lock property
    /// for Snapshot is moot here.
    ///
    /// `deps` is already snapshot-collected by the caller (under the
    /// PredicateSet Mutex), so this method holds no `Mutex` guard.
    pub fn predicate_conflicts_batch(
        &self,
        deps: &[crate::predicate_set::PredicateDep],
        snapshot: u64,
    ) -> Option<usize> {
        debug_assert!(!deps.is_empty(), "caller guards the empty case");
        // Single shared EBR guard for the ENTIRE validation — not one per dep.
        let guard = scc::ebr::Guard::new();
        let last = self.last_committed_version.load(Ordering::Acquire);
        if last <= snapshot {
            // Fast path: no new commits since snapshot.
            return None;
        }
        let start = snapshot.saturating_add(1);
        // Inverted loop: walk the window ONCE, test ALL deps per record.
        // Short-circuit on the first (record, dep) pair that conflicts.
        for (_v, rec) in self.commit_write_log.range(start..=last, &guard) {
            for (idx, dep) in deps.iter().enumerate() {
                if record_conflicts(rec, dep) {
                    return Some(idx);
                }
            }
        }
        None
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
    /// Decoupled from [`last_committed`](Self::last_committed) since the
    /// P1d-2b cutover: the commit ack-path publishes visibility
    /// (`last_committed`) BEFORE the value lands in history, and the
    /// background drainer's [`mark_durable`](Self::mark_durable) advances
    /// this watermark asynchronously once `replay_v2_entry` has written
    /// the version's data + index ops into history. Under steady state
    /// `durable_watermark() <= last_committed()` and the gap is bounded
    /// by the drainer's lag (one drain pass). The truncation gate
    /// (drainer Phase C / F6 segment unlink) refuses to advance past
    /// this watermark, so a sealed WAL segment holding an un-drained
    /// version is never unlinked.
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
    /// # Contract — "durable" here means HISTORY-WRITTEN, not fsynced
    ///
    /// **Naming caveat (do not be misled by the name):** "durable" in
    /// this method's name and in [`durable_watermark`](Self::durable_watermark)
    /// means "the version's data + index ops have been written into the
    /// history log (which on buffered backends lands in the OS page cache
    /// / store write-back buffer, NOT necessarily fsynced to disk)". A
    /// real on-disk fsync of history happens only later, at the truncation
    /// gate's checkpoint flush (see `InternerManager::persist`'s CRIT-2
    /// `info_store.flush()` and `RepoInstance::flush_buffers`). The name
    /// is retained because it is on every call site and a rename is a
    /// separate, higher-risk refactor; this doc comment is the contract.
    ///
    /// Concretely, `mark_durable(v)` is called by the drainer's
    /// `drain_step` (Phase C) AFTER `replay_v2_entry` returns Ok for `v`.
    /// It is NOT called from the commit ack-path (which only publishes
    /// visibility); the drainer owns the durable-watermark advance.
    ///
    /// `State::Materialized` is reused as the "durable" terminal state for
    /// this tracker: there is no Aborted analogue (an aborted version was
    /// never written and is irrelevant to durability — but the contiguous
    /// watermark still needs to advance past it). Aborted versions are
    /// marked here too (see
    /// [`mark_durable_aborted`](Self::mark_durable_aborted)) so the
    /// contiguous prefix on the durable tracker matches the visibility
    /// tracker's contiguous prefix.
    ///
    /// Idempotent: marking the same version twice (or marking a version at
    /// or below the current watermark) is a no-op.
    pub fn mark_durable(&self, version: u64) {
        self.durable_completion
            .mark(version, crate::completion_tracker::State::Materialized);
        // D2 P1e: wake any backpressured committer waiting on durable progress.
        // `notify_waiters` wakes ALL currently-registered waiters without
        // storing a permit — paired with the committer registering its
        // `notified()` future BEFORE re-checking the gap (see
        // `apply_backpressure`), there is no lost-wakeup window.
        self.durable_progress.notify_waiters();
    }

    /// D2 P1e — a future that resolves on the next durable-progress signal.
    ///
    /// Backpressure callers register this BEFORE re-reading the gap so a
    /// concurrent [`mark_durable`](Self::mark_durable) cannot slip a wakeup
    /// between the gap check and the park (no lost wakeup). The returned future
    /// borrows the gate; await it (optionally under a timeout) and loop.
    pub fn durable_notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.durable_progress.notified()
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
        per_table: TFxMap::default(),
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
