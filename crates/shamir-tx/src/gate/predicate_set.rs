//! Per-transaction predicate / range read-set for SSI phantom
//! protection (Phase C — see `docs/dev-artifacts/roadmap/PHASE_C_SERIALIZABLE.md`).
//!
//! Mirrors the point-key `TxContext.read_set` (tx_context.rs): a
//! parallel append-only log of *predicate dependencies* captured at
//! read time, validated at commit against concurrent committers'
//! write-keys (see Phase C doc section 5). Populated ONLY under
//! `IsolationLevel::Serializable`; Snapshot pays nothing.
//!
//! Concurrency note (CLAUDE.md "Concurrency invariants", pillar 5): the
//! dep log is a lock-free `scc::TreeIndex<u64, PredicateDep>` keyed by a
//! monotonic per-instance `AtomicU64` sequence — NOT a
//! `std::sync::Mutex<Vec<_>>`. F-66 (#892) replaced the sibling
//! `TxContext::ri_barrier_tokens`'s `std::sync::Mutex<TFxSet<u64>>` with a
//! lock-free `scc::HashSet` for the same reason that applies verbatim here:
//! a `std::sync::Mutex` poisoned by a panic under the guard permanently
//! breaks every later access on the same `TxContext` (every
//! `.lock().unwrap()` panics). The executor runs a tx's queries serially,
//! so contention IS nil in practice — but "nil contention" was the OLD
//! justification for keeping a `Mutex`, and F-79 (#906) retired it: the
//! poisoning exposure is unacceptable even when contention is nil, and the
//! Serializable read path that populates this set is a hot path. The dep
//! log is therefore lock-free CAS-based (`scc::TreeIndex::insert_sync`),
//! identical in spirit to F-66's `ri_barrier_tokens` migration. Cardinality
//! (`len`/`is_empty`) is served by an `AtomicUsize` mirror —
//! `scc::TreeIndex::len()` is O(N) and banned by `clippy.toml`
//! `disallowed-methods` (CLAUDE.md pillar 3).

use bytes::Bytes;
use std::ops::Bound;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// One captured predicate dependency of a Serializable tx.
///
/// Pure data — shamir-tx stays ignorant of how index keys are
/// composed (same contract as [`UniqueGuard`](crate::UniqueGuard)).
/// Recorded by engine-side read hooks; validated at commit.
#[derive(Debug, Clone)]
pub enum PredicateDep {
    /// The read was served by a sorted index. The scan covered the
    /// physical key interval `[lo, hi]` in that index's posting
    /// key-space. `index_id` distinguishes which index's key-space
    /// `lo/hi` live in.
    IndexRange {
        table_token: u64,
        index_id: u64,
        lo: Bound<Bytes>,
        hi: Bound<Bytes>,
    },

    /// Full-table scan / a predicate no sorted index serves. ANY
    /// insert/update into this table by a concurrent committer is a
    /// conflict. Over-aborts; never misses a phantom.
    TableScan { table_token: u64 },
}

/// Per-tx predicate read-set — append-only, interior-mutable.
///
/// Lives next to [`TxContext::read_set`](crate::TxContext::read_set).
///
/// Backing store is a lock-free `scc::TreeIndex<u64, PredicateDep>` keyed by
/// a monotonic per-instance `AtomicU64` sequence, plus an `AtomicUsize`
/// length mirror. This replaces the historical `std::sync::Mutex<Vec<_>>`
/// (F-79 / #906), following F-66's (#892) `ri_barrier_tokens` precedent:
/// (a) append-only during execution and scan-only at commit — a sequence-
///     keyed `TreeIndex` is a lock-free append-only log with ordered
///     iteration, preserving the exact `Vec`-order semantics;
/// (b) entries are not keyed, so a synthetic monotonic key is used purely
///     to give the lock-free tree a unique slot per append;
/// (c) the executor runs a tx's queries serially so contention is nil —
///     but a `std::sync::Mutex` poisoned by ANY panic under the guard would
///     permanently break the `TxContext` (every later `.lock().unwrap()`
///     panics), which is unacceptable on the Serializable read path.
///     `scc::TreeIndex` is CAS-based and has no poisoning.
///
/// `len`/`is_empty` read the `AtomicUsize` mirror (O(1)) rather than the
/// O(N) `scc::TreeIndex::len()` (banned by `clippy.toml`). The mirror is
/// updated with a `Relaxed` fetch_add at every `push`; the structure is
/// single-task-per-instance (serial executor), so no cross-thread ordering
/// is required for correctness — the `TreeIndex`'s own CAS provides the
/// happens-before edge for the data.
pub struct PredicateSet {
    log: scc::TreeIndex<u64, PredicateDep>,
    next_seq: AtomicU64,
    len_mirror: AtomicUsize,
}

impl PredicateSet {
    /// Empty set. Zero-alloc: `scc::TreeIndex::new()` and the two atomics
    /// do not heap-allocate until the first push, so the always-present
    /// field stays zero-cost on Snapshot.
    pub fn new() -> Self {
        Self {
            log: scc::TreeIndex::new(),
            next_seq: AtomicU64::new(0),
            len_mirror: AtomicUsize::new(0),
        }
    }

    /// Append one dependency. Takes `&self` via interior mutability
    /// — load-bearing because the engine's tx-aware read paths hold
    /// the tx by shared ref (`Option<&TxContext>`).
    ///
    /// Lock-free CAS (`scc::TreeIndex::insert_sync`); no poisoning. The
    /// synthetic key is a monotonic `AtomicU64` fetch_add, guaranteeing a
    /// unique slot per append and preserving insertion-iteration order.
    pub fn push(&self, dep: PredicateDep) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        // Monotonic seq ⇒ no duplicate keys ⇒ insert_sync always Ok; the
        // returned Err variant is statically unreachable.
        let _ = self.log.insert_sync(seq, dep);
        self.len_mirror.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of recorded deps. O(1) via the `AtomicUsize` mirror.
    pub fn len(&self) -> usize {
        self.len_mirror.load(Ordering::Relaxed)
    }

    /// True if no deps recorded. O(1) via the `AtomicUsize` mirror.
    pub fn is_empty(&self) -> bool {
        self.len_mirror.load(Ordering::Relaxed) == 0
    }

    /// Iterate over recorded deps in insertion order, applying `f` to
    /// each. Used by commit-time predicate validation. The lock-free
    /// `scc::TreeIndex` reader runs under an EBR `Guard` confined to this
    /// synchronous call — no guard or lock is held across `.await`.
    pub fn with_iter<F: FnMut(&PredicateDep)>(&self, mut f: F) {
        let guard = scc::Guard::new();
        for (_seq, dep) in self.log.iter(&guard) {
            f(dep);
        }
    }

    /// Snapshot all recorded deps into a `Vec` in insertion order.
    ///
    /// Used by the inverted batch predicate-validation path
    /// (`RepoTxGate::predicate_conflicts_batch`): the deps are cloned
    /// out once under the EBR guard, then the guard is released before the
    /// commit-window scan — so no guard is held across the EBR-guarded
    /// tree range walk. The clone is O(P) in dep count and only fires
    /// on the Serializable + non-empty-predicate path.
    pub fn snapshot_deps(&self) -> Vec<PredicateDep> {
        let guard = scc::Guard::new();
        self.log
            .iter(&guard)
            .map(|(_seq, dep)| dep.clone())
            .collect()
    }
}

impl Default for PredicateSet {
    fn default() -> Self {
        Self::new()
    }
}

/// `SORTED_TAG` must match
/// `crates/shamir-engine/src/index/sorted_index_manager.rs:60`.
/// Kept local (rather than re-exported) so shamir-tx stays decoupled
/// from the engine crate. Pinned by `key_in_interval_prefix_tag_matches`
/// test.
pub const SORTED_TAG: u8 = 0x80;

/// Length of the physical key prefix that uniquely identifies one
/// sorted index (tag byte + 8-byte BE index id). See `entry_prefix`,
/// sorted_index_manager.rs:574.
pub const SORTED_PREFIX_LEN: usize = 1 + 8;

/// Build the 9-byte sorted-index prefix `SORTED_TAG || index_id.to_be_bytes()`.
/// Mirror of the private `SortedIndexManager::entry_prefix` (:574) — kept
/// here so the predicate layer can construct/check bounds without
/// re-entering the engine.
#[inline]
fn sorted_prefix_bytes(index_id: u64) -> [u8; SORTED_PREFIX_LEN] {
    let mut p = [0u8; SORTED_PREFIX_LEN];
    p[0] = SORTED_TAG;
    p[1..].copy_from_slice(&index_id.to_be_bytes());
    p
}

/// True iff `posting_key` is a sorted-index posting for `index_id` AND
/// falls inside the byte interval `[lo, hi]`.
///
/// Layout assumed (sorted_index_manager.rs:582 / :15 doc block):
///   `posting_key = SORTED_TAG (1) || index_id BE8 (8) || encoded_value || rid (16)`
///
/// Bounds were constructed in the same physical-byte space by
/// `range_bounds` (sorted_index_manager.rs:516) and stored in
/// `PredicateDep::IndexRange { lo, hi }` (doc section 3.1).
///
/// Comparison is plain lexicographic on byte slices — exactly the
/// order under which the sort_codec was designed (see
/// shamir-types/src/core/sort_codec.rs doc block, lines 14-32).
///
/// `Bound` semantics:
///   - `Unbounded`    — no constraint on that side.
///   - `Included(b)`  — posting_key compares `>= b` (lo) or `<= b` (hi).
///   - `Excluded(b)`  — strict `>` (lo) or strict `<` (hi).
///
/// Cost: O(min(|posting_key|, |bound|)) — one or two memcmp-style compares.
pub fn key_in_interval(
    posting_key: &[u8],
    index_id: u64,
    lo: &Bound<Bytes>,
    hi: &Bound<Bytes>,
) -> bool {
    // Step 1: every dep is scoped to one index — the posting must live
    // in that index's keyspace.
    let prefix = sorted_prefix_bytes(index_id);
    if posting_key.len() < SORTED_PREFIX_LEN || posting_key[..SORTED_PREFIX_LEN] != prefix {
        return false;
    }

    // Step 2: lower bound.
    match lo {
        Bound::Unbounded => {}
        Bound::Included(b) => {
            if posting_key < b.as_ref() {
                return false;
            }
        }
        Bound::Excluded(b) => {
            if posting_key <= b.as_ref() {
                return false;
            }
        }
    }

    // Step 3: upper bound.
    match hi {
        Bound::Unbounded => {}
        Bound::Included(b) => {
            if posting_key > b.as_ref() {
                return false;
            }
        }
        Bound::Excluded(b) => {
            if posting_key >= b.as_ref() {
                return false;
            }
        }
    }
    true
}

impl std::fmt::Debug for PredicateSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Lock-free: cardinality is an O(1) atomic load, so there is no
        // contended-guard `try_lock` fallback to worry about.
        f.debug_struct("PredicateSet")
            .field("len", &self.len_mirror.load(Ordering::Relaxed))
            .finish()
    }
}
