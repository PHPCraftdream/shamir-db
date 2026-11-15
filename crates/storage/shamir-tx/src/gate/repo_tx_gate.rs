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

use shamir_collections::THasher;
use shamir_wal::WalEntryV2;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use crate::completion_tracker::CompletionTracker;
use crate::pending_commit::PendingCommit;

#[path = "../version/visibility_metrics/resume_access.rs"]
mod resume_impl;

mod core;
mod footprint;
mod orphan;
mod snapshot;

pub use footprint::{CommitWriteRecord, TableWriteFootprint, Unresolved};
#[cfg(test)]
pub(crate) use snapshot::min_alive_probe;
pub use snapshot::{SnapshotGuard, VisibleOutcome};

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

    /// Ordered index over the DISTINCT pinned snapshot versions (refcount > 0
    /// in `active_snapshots`). `min_alive` reads its first key as a bounded
    /// O(log n) lookup instead of scanning the whole refcount map.
    snapshot_versions: Arc<scc::TreeIndex<u64, ()>>,

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
    /// `crates/engine/shamir-engine/src/tx/finalize.rs:11`); `enqueue_pending` /
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
    /// `Arc` so a [`crate::version_guard::VersionGuard`] can hold a clone and own the terminal
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

mod progress;

pub use progress::{build_footprint_from_tx, record_conflicts};
