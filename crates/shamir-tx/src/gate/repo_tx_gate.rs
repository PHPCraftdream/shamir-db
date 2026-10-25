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
use shamir_wal::{WalEntryV2, WalTicket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::completion_tracker::CompletionTracker;
use crate::pending_commit::PendingCommit;
use crate::version_guard::VersionGuard;

// ── Phase C: commit-write log structs ──────────────────────────────

#[path = "../version/visibility_metrics/resume_access.rs"]
mod resume_impl;

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

/// Ticket and replay entry transferred to a resolver; taking it does not settle
/// either tracker. A tuple keeps this additive handover API directly usable.
pub type Unresolved = (Arc<WalTicket>, Arc<WalEntryV2>);

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

    /// Highest fully published version; readers see only versions ≤ this.
    /// `Arc` lets `VersionGuard` advance it without a gate back-reference.
    last_committed_version: Arc<AtomicU64>,
    pub(crate) receipt_native: AtomicBool,
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
    ///
    /// **F-79 (#906) — sanctioned `std::sync::Mutex` exception (dead
    /// scaffolding).** This is the ONLY remaining `std::sync::Mutex` on a
    /// runtime struct in shamir-tx, and it is sanctioned because the field
    /// is **dead**: the group-commit leader/follower path that would have
    /// used it (`tx/group_commit.rs`) was removed in F-54 (#865 — see
    /// `crates/shamir-engine/src/tx/finalize.rs:11`); `enqueue_pending` /
    /// `drain_pending` have ZERO callers in the live commit path or any
    /// test (`rg enqueue_pending|drain_pending` resolves to the two
    /// definitions below + a doc reference in `pending_commit.rs`), and
    /// the 2026-07-06 concurrency-engine audit (§1) independently flags
    /// this as dead. The contention model is therefore not "nil" but
    /// **nonexistent**: the mutex is never locked on any code path, hot
    /// or cold, so the CLAUDE.md "banned in hot paths" rule (which
    /// targets live hot paths) does not reach it. Cross-referenced from
    /// CLAUDE.md "Concurrency invariants" so the exception is
    /// discoverable from the normative rule, not just from this field.
    ///
    /// A std (non-async) `Mutex` is retained rather than a lock-free
    /// structure because (1) it carries zero runtime cost while dead
    /// (never locked), (2) the only two accessors are `push`/`drain-all`
    /// which a future group-commit revival would need to re-audit for its
    /// ACTUAL contention model anyway, and (3) `PendingCommit` is not
    /// `Clone` (it owns a `TxContext` + `oneshot::Sender` +
    /// `OwnedMutexGuard`s), so a lock-free `scc::TreeIndex`-backed drain
    /// cannot return an owned `Vec<PendingCommit>` without a per-entry
    /// remove loop — unjustified complexity for code with no callers.
    /// If/when group-commit is revived, this field MUST be re-evaluated
    /// against F-66/F-79's poisoning precedent before it takes a live
    /// call.
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

    orphan_entries: Arc<scc::HashMap<u64, Arc<WalEntryV2>, THasher>>,
    wal_unresolved: Arc<scc::HashMap<u64, Unresolved, THasher>>,
    requires_reopen: Arc<AtomicBool>,
    /// Set only when a failed write's compensating cleanup could not settle
    /// its versions; new writes are refused (unlike `requires_reopen`, which
    /// still lets a write commit and report a pending-visibility exception).
    writes_fenced: Arc<AtomicBool>,
    orphaned_post_wal: Arc<scc::HashMap<u64, (), THasher>>,
    orphan_max: Arc<AtomicU64>,
    active_post_wal: Arc<scc::HashMap<u64, (), THasher>>,

    // Diagnostic-only RCU slot: bounded to one observation, never touched by
    // allocation/commit/Drop. Lazy sampling adds no work to those hot paths.
    oldest_observation: arc_swap::ArcSwapOption<(u64, tokio::time::Instant)>,
    warning_epoch: tokio::time::Instant,
    next_visibility_warning_ns: AtomicU64,
    #[cfg(test)]
    visibility_warning_count: AtomicU64,

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
            match self.snapshots.entry_sync(v) {
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

/// Result of waiting for the contiguous visibility prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleOutcome {
    Visible,
    ReopenRequired,
}

impl RepoTxGate {
    /// Create a new gate, optionally seeded from durable markers read
    /// on repo open.
    pub fn new(last_committed: u64, next_tx_id_seed: u64) -> Self {
        Self {
            commit_mutex: tokio::sync::Mutex::new(()),
            version_counter: AtomicU64::new(last_committed),
            last_committed_version: Arc::new(AtomicU64::new(last_committed)),
            receipt_native: AtomicBool::new(false),
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

    /// cancel-safe: yes — the `entry_sync` calls in `bump_refcount`
    /// (formerly `entry_async`; switched to the synchronous accessor to
    /// close the same-class whole-runtime deadlock hazard fixed for the
    /// `cells` map in #589 / commit `7a4abf62`) are CAS-based and either
    /// complete or leave the map unchanged on cancellation. The function
    /// no longer suspends. If the future is dropped after a registration
    /// but before returning the guard, the snapshot entry leaks (no Drop
    /// runs); since callers always await this to completion or never call
    /// it, in practice this is cancel-safe.
    ///
    /// Register Snapshot/Serializable reads at the contiguous completion watermark W.
    /// Eager publication can skip unsettled versions, which would later appear
    /// inside a snapshot. Latest-state reads still use `last_committed`.
    /// Registration is protected by the A10 opening barrier and rechecked
    /// before returning the RAII guard.
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

    /// Register exactly `pin`, without reconciling it with the published
    /// marker or the contiguous completion watermark W.
    ///
    /// Intended for DDL callers that establish a per-table cut under a
    /// writer-drain barrier. The caller must ensure the requested history
    /// has not already been reclaimed before registration begins.
    ///
    /// Uses the A10 opening barrier and the same refcount/Drop ownership as
    /// `open_snapshot`. A pin above W must not become the GC minimum by
    /// itself: GC must also include W in its floor (the separate GC-floor
    /// remediation). This method does not settle versions or advance W.
    ///
    /// cancel-safe: yes — `bump_refcount` has no suspension points, so the
    /// opening barrier, registration, and guard construction finish in one
    /// poll. Dropping an unpolled future registers nothing.
    pub async fn open_snapshot_at(&self, pin: u64) -> SnapshotGuard {
        let _barrier = OpeningBarrier::new(&self.active_snapshots_opening);
        // Allocate before registration so the guard owns every bumped refcount.
        let registered = vec![pin];
        self.bump_refcount(pin).await;
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
    /// BEFORE the first `completion().watermark()` read and decremented AFTER the
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
    /// 1. Read the floor `v0 = completion().watermark()`.
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
        // the reader's subsequent Acquire load of `completion().watermark()`.
        let _barrier = OpeningBarrier::new(&self.active_snapshots_opening);

        const MAX_RETRIES: usize = 4;
        let mut registered = Vec::with_capacity(2);
        let mut current = self.completion().watermark();
        loop {
            self.bump_refcount(current).await;
            registered.push(current);
            let next = self.completion().watermark();
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
    /// `entry_sync` so the read-modify-write is race-free against
    /// concurrent openers and droppers.
    ///
    /// DEADLOCK FIX (same class as #589 / `cells` map, commit `7a4abf62`):
    /// `entry_sync`, NOT `entry_async`. This map is read on EVERY
    /// transaction end via the SYNCHRONOUS `SnapshotGuard::drop` →
    /// `entry_sync(v)` (`active_snapshots.entry_sync`), which PARKS the
    /// calling OS thread while the bucket is exclusively locked (scc's
    /// synchronous accessor parks the worker until the lock is free), and
    /// also scanned via the SYNCHRONOUS `min_alive` / `active_snapshots_empty`
    /// GC-tick paths (`iter_sync` / `is_empty`). `entry_async`'s wait path
    /// is lock-HANDOFF: on release, saa GRANTS the exclusive bucket lock to
    /// the suspended waiter TASK, which then holds it while sitting in
    /// tokio's run queue until next polled. This map is keyed by version,
    /// and EVERY concurrent `open_snapshot` targets the SAME key — the
    /// CURRENT `last_committed` version — so all contention funnels onto
    /// ONE bucket by construction. If, in the lock-handoff window, every
    /// runtime worker thread runs a `SnapshotGuard::drop` (every tx end)
    /// or a GC-tick `iter_sync` on that same bucket, all workers park and
    /// the lock-owning `bump_refcount` task can never be polled again →
    /// whole-runtime deadlock. With `entry_sync` the exclusive lock is
    /// only ever held by a RUNNING thread for a few instructions (a
    /// saturating increment), so every synchronous-reader wait is
    /// structurally bounded — the deadlock window is closed by
    /// construction, not papered over with a longer timeout. The fn stays
    /// `async` to keep its call sites / public shape unchanged; it no
    /// longer suspends.
    async fn bump_refcount(&self, version: u64) {
        match self.active_snapshots.entry_sync(version) {
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

    pub fn orphaned_post_wal_versions(&self) -> Vec<u64> {
        let mut versions = Vec::new();
        self.orphaned_post_wal.iter_sync(|version, _| {
            versions.push(*version);
            true
        });
        versions.sort_unstable();
        versions
    }

    /// Retained replay entry, including frames outside the drainer's window.
    pub fn orphan_entry(&self, version: u64) -> Option<Arc<WalEntryV2>> {
        self.orphan_entries
            .read_sync(&version, |_, entry| Arc::clone(entry))
    }

    /// Ascending snapshot; concurrent transfers may appear on the next scan.
    pub fn unresolved_wal_versions(&self) -> Vec<u64> {
        let mut versions = Vec::new();
        self.wal_unresolved.iter_sync(|version, _| {
            versions.push(*version);
            true
        });
        versions.sort_unstable();
        versions
    }

    pub fn unresolved_wal(&self, version: u64) -> Option<Unresolved> {
        self.wal_unresolved
            .read_sync(&version, |_, value| value.clone())
    }

    pub fn register_written_orphan(&self, version: u64, entry: Arc<WalEntryV2>) {
        let _ = self.orphan_entries.insert_sync(version, entry);
        let _ = self.orphaned_post_wal.insert_sync(version, ());
        self.orphan_max.fetch_max(version, Ordering::AcqRel);
    }

    /// Transfer resolution ownership without settling either tracker.
    /// The caller must retain the ticket/entry until its physical fate is known.
    pub fn take_unresolved(&self, version: u64) -> Option<Unresolved> {
        self.wal_unresolved
            .remove_sync(&version)
            .map(|(_, value)| value)
    }

    /// Settle a version proven unwritten. Never use for PENDING or UNKNOWN.
    /// The resolver must first take ownership with `take_unresolved`.
    pub fn abort_unwritten(&self, version: u64) {
        self.completion
            .mark(version, crate::completion_tracker::State::Aborted);
        self.durable_completion
            .mark(version, crate::completion_tracker::State::Aborted);
        self.sync_last_committed_from_watermark();
    }

    /// Fail-stop signal only; admission wiring belongs to the later engine stage.
    pub fn set_requires_reopen(&self) {
        self.requires_reopen.store(true, Ordering::Release);
    }

    pub fn requires_reopen(&self) -> bool {
        self.requires_reopen.load(Ordering::Acquire)
    }

    /// Fail-stop: refuse new writes and require reopen.
    pub fn fence_writes(&self) {
        self.writes_fenced.store(true, Ordering::Release);
        self.set_requires_reopen();
    }

    pub fn writes_fenced(&self) -> bool {
        self.writes_fenced.load(Ordering::Acquire)
    }

    /// Highest version ever registered as a post-WAL orphan (zero if none).
    /// Monotone scan bound; forgetting an orphan does not lower it.
    pub fn orphan_max(&self) -> u64 {
        self.orphan_max.load(Ordering::Acquire)
    }

    /// Whether a live commit owns this WAL-accepted version's materialization.
    /// The drainer must stop at it rather than replaying concurrently.
    pub fn is_active_post_wal(&self, version: u64) -> bool {
        self.active_post_wal.contains_sync(&version)
    }

    /// Whether a dropped post-WAL guard transferred this version to replay.
    pub fn is_orphaned_post_wal(&self, version: u64) -> bool {
        self.orphaned_post_wal.contains_sync(&version)
    }

    /// Mark a registered orphan Materialized and advance visibility after
    /// replay succeeds. Does not mark durability or forget membership.
    pub fn complete_orphaned_post_wal(&self, version: u64) {
        if self.is_orphaned_post_wal(version) {
            self.completion
                .mark(version, crate::completion_tracker::State::Materialized);
            self.sync_last_committed_from_watermark();
        }
    }

    /// Remove settled orphan membership after replay and visibility completion.
    /// The drainer separately marks durability; orphan_max remains monotone.
    pub fn forget_orphaned_post_wal(&self, version: u64) {
        let _ = self.orphaned_post_wal.remove_sync(&version);
        let _ = self.orphan_entries.remove_sync(&version);
    }

    /// Publish: update `last_committed_version` atomically.
    ///
    /// Legacy plain-store fixture hook; does not settle completion. Production
    /// publishers use `raise_published`, and transaction snapshots read W.
    pub fn publish_committed(&self, version: u64) {
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

    /// Minimum GC floor: `min(active snapshots U {completion watermark})`.
    /// The contiguous completion watermark W is an always-live snapshot, even
    /// when no snapshots are registered or all explicit pins are above W.
    /// `last_committed` is eager and non-contiguous: pruning to it could lose
    /// history needed at W and the commit-log window `(W, last]` needed by SSI.
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
        let mut min = self.completion().watermark();
        self.active_snapshots.iter_sync(|k, _| {
            if *k < min {
                min = *k;
            }
            true
        });
        min
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
}

mod progress;

pub use progress::{build_footprint_from_tx, record_conflicts};
