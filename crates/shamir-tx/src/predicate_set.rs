//! Per-transaction predicate / range read-set for SSI phantom
//! protection (Phase C — see `docs/roadmap/PHASE_C_SERIALIZABLE.md`).
//!
//! Mirrors the point-key `TxContext.read_set` (tx_context.rs): a
//! parallel append-only log of *predicate dependencies* captured at
//! read time, validated at commit against concurrent committers'
//! write-keys (see Phase C doc section 5). Populated ONLY under
//! `IsolationLevel::Serializable`; Snapshot pays nothing.
//!
//! Concurrency note (CLAUDE.md "Concurrency invariants" + Phase C
//! doc section 3.1): a plain `std::sync::Mutex<Vec<_>>` is acceptable here,
//! and ONLY here, because the guard is never held across `.await` and
//! the container is never read on a hot non-tx path. The executor
//! runs a tx's queries serially, so contention is nil.

use bytes::Bytes;
use std::ops::Bound;
use std::sync::Mutex;

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
/// `Mutex<Vec<_>>` (not `scc::HashMap`) because (a) it is
/// append-only during execution and scan-only at commit, (b)
/// entries are not keyed, (c) the executor runs a tx's queries
/// serially so contention is nil — the lock is always taken
/// uncontended. A plain `std::sync::Mutex` is acceptable ONLY here
/// because it is never held across `.await` and never on a hot
/// non-tx path.
pub struct PredicateSet {
    inner: Mutex<Vec<PredicateDep>>,
}

impl PredicateSet {
    /// Empty set. Zero-alloc: `Vec::new()` does not heap-allocate
    /// until the first push, so the always-present field stays
    /// zero-cost on Snapshot.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
        }
    }

    /// Append one dependency. Takes `&self` via interior mutability
    /// — load-bearing because the engine's tx-aware read paths hold
    /// the tx by shared ref (`Option<&TxContext>`).
    pub fn push(&self, dep: PredicateDep) {
        self.inner.lock().unwrap().push(dep);
    }

    /// Number of recorded deps.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// True if no deps recorded.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }

    /// Iterate over recorded deps under the lock, applying `f` to
    /// each. Used by commit-time predicate validation — keeps the
    /// lock guard from leaking across `.await` by confining its
    /// scope to a synchronous closure.
    pub fn with_iter<F: FnMut(&PredicateDep)>(&self, mut f: F) {
        let g = self.inner.lock().unwrap();
        for dep in g.iter() {
            f(dep);
        }
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
        match self.inner.try_lock() {
            Ok(g) => f
                .debug_struct("PredicateSet")
                .field("len", &g.len())
                .finish(),
            Err(_) => f
                .debug_struct("PredicateSet")
                .field("len", &"<locked>")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_types::core::sort_codec;
    use std::ops::Bound::{Excluded, Included, Unbounded};

    // ---- helpers ----------------------------------------------------------

    fn posting(index_id: u64, encoded_value: &[u8], rid_byte: u8) -> Vec<u8> {
        let mut k = Vec::with_capacity(SORTED_PREFIX_LEN + encoded_value.len() + 16);
        k.push(SORTED_TAG);
        k.extend_from_slice(&index_id.to_be_bytes());
        k.extend_from_slice(encoded_value);
        k.extend_from_slice(&[rid_byte; 16]);
        k
    }

    fn enc_i(v: i64) -> Vec<u8> {
        let mut b = Vec::new();
        sort_codec::encode_i64(&mut b, v);
        b
    }
    fn enc_s(s: &str) -> Vec<u8> {
        let mut b = Vec::new();
        sort_codec::encode_str(&mut b, s);
        b
    }
    fn bound_with_prefix(index_id: u64, tail: &[u8]) -> Bytes {
        let mut b = Vec::with_capacity(SORTED_PREFIX_LEN + tail.len());
        b.push(SORTED_TAG);
        b.extend_from_slice(&index_id.to_be_bytes());
        b.extend_from_slice(tail);
        Bytes::from(b)
    }
    fn full_max_upper(index_id: u64) -> Bytes {
        let mut b = Vec::with_capacity(SORTED_PREFIX_LEN + 64);
        b.push(SORTED_TAG);
        b.extend_from_slice(&index_id.to_be_bytes());
        b.extend_from_slice(&[0xFFu8; 64]);
        Bytes::from(b)
    }

    // ---- key_in_interval tests -------------------------------------------

    #[test]
    fn key_in_interval_unbounded_matches_within_index() {
        let k = posting(7, &enc_i(42), 0xAA);
        assert!(key_in_interval(&k, 7, &Unbounded, &Unbounded));
    }

    #[test]
    fn key_in_interval_rejects_wrong_index_id() {
        let k = posting(7, &enc_i(42), 0xAA);
        assert!(!key_in_interval(&k, 8, &Unbounded, &Unbounded));
    }

    #[test]
    fn key_in_interval_rejects_short_key() {
        let k = vec![SORTED_TAG, 0, 0, 0];
        assert!(!key_in_interval(&k, 0, &Unbounded, &Unbounded));
    }

    #[test]
    fn key_in_interval_rejects_non_sorted_tag() {
        let mut k = posting(7, &enc_i(42), 0xAA);
        k[0] = 0x00;
        assert!(!key_in_interval(&k, 7, &Unbounded, &Unbounded));
    }

    #[test]
    fn key_in_interval_gte_inclusive_lo() {
        let lo = bound_with_prefix(7, &enc_i(30));
        let hi = full_max_upper(7);
        assert!(key_in_interval(
            &posting(7, &enc_i(30), 0x00),
            7,
            &Included(lo.clone()),
            &Included(hi.clone())
        ));
        assert!(key_in_interval(
            &posting(7, &enc_i(31), 0x00),
            7,
            &Included(lo.clone()),
            &Included(hi.clone())
        ));
        assert!(!key_in_interval(
            &posting(7, &enc_i(29), 0xFF),
            7,
            &Included(lo),
            &Included(hi)
        ));
    }

    #[test]
    fn key_in_interval_gt_excludes_boundary_value_across_all_rids() {
        let mut bound_tail = enc_i(30);
        bound_tail.extend_from_slice(&[0xFFu8; 16]);
        let lo = bound_with_prefix(7, &bound_tail);
        let hi = full_max_upper(7);
        for rid in [0x00u8, 0x7F, 0xFF] {
            assert!(
                !key_in_interval(
                    &posting(7, &enc_i(30), rid),
                    7,
                    &Excluded(lo.clone()),
                    &Included(hi.clone())
                ),
                "rid byte {rid:#x} at boundary should be excluded"
            );
        }
        assert!(key_in_interval(
            &posting(7, &enc_i(31), 0x00),
            7,
            &Excluded(lo),
            &Included(hi)
        ));
    }

    #[test]
    fn key_in_interval_lt_excludes_boundary_value() {
        let hi = bound_with_prefix(7, &enc_i(20));
        let lo = bound_with_prefix(7, b"");
        assert!(!key_in_interval(
            &posting(7, &enc_i(20), 0x00),
            7,
            &Included(lo.clone()),
            &Excluded(hi.clone())
        ));
        assert!(key_in_interval(
            &posting(7, &enc_i(19), 0xFF),
            7,
            &Included(lo),
            &Excluded(hi)
        ));
    }

    #[test]
    fn key_in_interval_between_inclusive() {
        let lo = bound_with_prefix(7, &enc_i(10));
        let mut hi_tail = enc_i(20);
        hi_tail.extend_from_slice(&[0xFFu8; 16]);
        let hi = bound_with_prefix(7, &hi_tail);
        for v in [10, 15, 20] {
            assert!(
                key_in_interval(
                    &posting(7, &enc_i(v), 0x42),
                    7,
                    &Included(lo.clone()),
                    &Included(hi.clone())
                ),
                "{v} should be in [10,20]"
            );
        }
        for v in [9, 21] {
            assert!(
                !key_in_interval(
                    &posting(7, &enc_i(v), 0x42),
                    7,
                    &Included(lo.clone()),
                    &Included(hi.clone())
                ),
                "{v} should be out of [10,20]"
            );
        }
    }

    #[test]
    fn key_in_interval_string_eq_degenerate_range() {
        let lo = bound_with_prefix(7, &enc_s("bob"));
        let mut hi_tail = enc_s("bob");
        hi_tail.extend_from_slice(&[0xFFu8; 16]);
        let hi = bound_with_prefix(7, &hi_tail);
        assert!(key_in_interval(
            &posting(7, &enc_s("bob"), 0x00),
            7,
            &Included(lo.clone()),
            &Included(hi.clone())
        ));
        assert!(!key_in_interval(
            &posting(7, &enc_s("bo"), 0xFF),
            7,
            &Included(lo.clone()),
            &Included(hi.clone())
        ));
        assert!(!key_in_interval(
            &posting(7, &enc_s("boby"), 0x00),
            7,
            &Included(lo),
            &Included(hi)
        ));
    }

    #[test]
    fn key_in_interval_prefix_tag_matches_engine_constant() {
        assert_eq!(SORTED_TAG, 0x80);
    }

    // ---- PredicateSet tests (pre-existing) --------------------------------

    #[test]
    fn new_predicate_set_is_empty() {
        let ps = PredicateSet::new();
        assert!(ps.is_empty());
        assert_eq!(ps.len(), 0);
    }

    #[test]
    fn push_and_len() {
        let ps = PredicateSet::new();
        ps.push(PredicateDep::TableScan { table_token: 7 });
        ps.push(PredicateDep::IndexRange {
            table_token: 7,
            index_id: 42,
            lo: Bound::Included(Bytes::from_static(b"\x00")),
            hi: Bound::Unbounded,
        });
        assert_eq!(ps.len(), 2);
        let mut seen = 0usize;
        ps.with_iter(|_| seen += 1);
        assert_eq!(seen, 2);
    }

    #[test]
    fn push_through_shared_ref() {
        let ps = PredicateSet::new();
        let r: &PredicateSet = &ps;
        r.push(PredicateDep::TableScan { table_token: 1 });
        assert_eq!(ps.len(), 1);
    }
}
