//! In-memory versioned overlay for the `(key, version) → value` window.
//!
//! [`VersionedOverlay`] holds committed values that have been made visible
//! (cell version published, `CompletionTracker::mark(Materialized)`) but not
//! yet drained to the durable `history` store. It sits between `RecordCell`
//! and the on-disk version log, giving instant ack-path visibility while the
//! background drain-leader flushes batches asynchronously.
//!
//! **Indexes and concurrency.** A key-major [`scc::TreeIndex`] supports
//! reads and ordered scans; a version-major tree supports GC. An
//! [`scc::HashMap`] coordinates mutations using synchronous bucket guards,
//! serializing mutations that share a bucket across all three indexes.
//!
//! **Wired in (P1a built it; P1b–P1e integrated it).** [`MvccStore`]
//! (`mvcc_store/mod.rs`) holds this overlay and consults it on both the
//! read path (`newest_visible` / exact-version `get`) and the write path
//! (`insert` on commit, `remove`/`gc_upto` on reclaim), with
//! `drain_to_history` flushing its entries to the durable history log via
//! `iter_all_le`/`snapshot_le`. See also
//! `crates/shamir-tx/src/tests/versioned_overlay_tests.rs`.

use std::ops::Bound::{Excluded, Unbounded};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use scc::{HashMap, TreeIndex};
use shamir_collections::THasher;
use shamir_storage::types::RecordKey;

/// Composite key for the overlay B+ tree: `(record_key, version)`.
///
/// [`RecordKey`] implements `Ord` lexicographically over the byte slice
/// (a hand-written impl — never a derive over the SSO `Repr`), and tuple
/// `Ord` compares component-wise (first `RecordKey`, then `u64`), so all
/// versions of a single key are contiguous in the tree and ordered by
/// ascending version. Keying on `RecordKey` (not `Bytes`) lets an already
/// alloc-free `RecordKey` value from the commit path land in the tree with
/// zero heap round-trip (task #532; the `RecordId` shape — 16 bytes — stays
/// inline). Composing the tuple `Ord`/`Eq` derive over `RecordKey`'s own
/// hand-written impls is sound: it does NOT derive over `KeyBytes`'s inner
/// `Repr` (#503 rule).
type OverlayKey = (RecordKey, u64);

struct OverlayEntry {
    value: Bytes,
    bytes: usize,
}

/// In-memory versioned overlay.
///
/// Holds `(key, version) → value` entries for the window
/// `(durable_watermark, visibility_watermark]`. Tombstones are stored as
/// empty `Bytes` (same convention as `history`), so `get` returning
/// `Some(Bytes::new())` means "deleted at that version" — the caller
/// (resolve_read) decides semantics.
///
/// Readers use the key-major tree; mutations coordinate via synchronous bucket guards.
pub struct VersionedOverlay {
    /// Lock-free B+ tree: `(record_key, version) → value`.
    tree: TreeIndex<OverlayKey, Arc<OverlayEntry>>,
    by_version: TreeIndex<(u64, RecordKey), Arc<OverlayEntry>>,
    mutations: HashMap<OverlayKey, Arc<OverlayEntry>, THasher>,
    /// Estimated footprint, accounted once per live entry.
    byte_size: AtomicUsize,
    /// Entry count maintained atomically (TreeIndex has no O(1) len).
    count: AtomicUsize,
}

// Nominal per-entry overhead for the shared entry and three indexes; this is
// an estimate, not an exact accounting of allocations or index capacity.
const PER_ENTRY_OVERHEAD: usize = 64;

impl VersionedOverlay {
    /// Create an empty overlay.
    #[inline]
    pub fn new() -> Self {
        Self {
            tree: TreeIndex::new(),
            by_version: TreeIndex::new(),
            mutations: HashMap::with_hasher(THasher::default()),
            byte_size: AtomicUsize::new(0),
            count: AtomicUsize::new(0),
        }
    }

    /// Insert a versioned value into the overlay.
    ///
    /// `version` is unique per commit — a duplicate insert (same key +
    /// version) is treated as an idempotent replay and silently ignored.
    /// The first writer wins; this is correct because the same version
    /// always carries the same payload (WAL determinism).
    pub fn insert(&self, key: RecordKey, version: u64, value: Bytes) {
        let composite = (key, version);
        if let scc::hash_map::Entry::Vacant(slot) = self.mutations.entry_sync(composite.clone()) {
            let entry = Arc::new(OverlayEntry {
                bytes: composite.0.len() + value.len() + PER_ENTRY_OVERHEAD,
                value,
            });
            let _guard = slot.insert_entry(Arc::clone(&entry));
            self.byte_size.fetch_add(entry.bytes, Ordering::Relaxed);
            self.count.fetch_add(1, Ordering::Relaxed);
            assert!(self
                .by_version
                .insert_sync((version, composite.0.clone()), Arc::clone(&entry))
                .is_ok());
            assert!(self.tree.insert_sync(composite, entry).is_ok());
        }
    }

    /// Exact-version point lookup: returns the value at `(key, version)`.
    ///
    /// This is the direct-path read used when `RecordCell.version` points
    /// at a version still in the overlay.
    pub fn get(&self, key: &[u8], version: u64) -> Option<Bytes> {
        self.peek(key, version)
    }

    /// Remove the exact `(key, version)` entry, if present.
    ///
    /// P1c: the symmetric half of [`Self::insert`] on the dual-write reclaim
    /// path. When `vacuum_key` / `gc_below` / `purge_below_ts` reclaim a version
    /// from the durable `history` log, the SAME `(key, version)` must leave the
    /// overlay in lockstep — otherwise the overlay would keep serving a value
    /// that history has dropped, breaking the overlay-mirrors-history invariant.
    ///
    /// Idempotent: removing an absent entry is a no-op (the value was already
    /// drained or never inserted).
    pub fn remove(&self, key: &[u8], version: u64) {
        let composite = (RecordKey::from_slice(key), version);
        self.remove_if_current(&composite, None);
    }

    fn remove_if_current(&self, composite: &OverlayKey, expected: Option<&Arc<OverlayEntry>>) {
        if let scc::hash_map::Entry::Occupied(slot) = self.mutations.entry_sync(composite.clone()) {
            let entry = slot.get();
            if expected.is_some_and(|candidate| !Arc::ptr_eq(candidate, entry)) {
                return;
            }
            let bytes = entry.bytes;
            assert!(self.tree.remove_sync(composite));
            assert!(self
                .by_version
                .remove_sync(&(composite.1, composite.0.clone())));
            self.byte_size.fetch_sub(bytes, Ordering::Relaxed);
            self.count.fetch_sub(1, Ordering::Relaxed);
            let _ = slot.remove_entry();
        }
    }

    /// Newest version `≤ max_version` for `key`, or `None`.
    ///
    /// Range-scans `(key, 0)..=(key, max_version)` in the B+ tree and
    /// returns the last (highest-version) entry. This is the fallback-path
    /// read used when cells report a version that may have been GC'd or
    /// when a snapshot needs the latest committed value.
    pub fn newest_visible(&self, key: &[u8], max_version: u64) -> Option<(u64, Bytes)> {
        // Inline-cheap for the 16-byte `RecordId` shape; the range bounds are
        // `(RecordKey, version)` tuples over the tree's key type.
        let key_rk = RecordKey::from_slice(key);
        let lo = (key_rk.clone(), 0u64);
        let hi = (key_rk, max_version);

        let guard = scc::Guard::new();
        // TreeIndex::range returns entries in ascending key order.
        // We want the *last* entry in [lo..=hi], i.e. the newest version.
        self.tree
            .range(lo..=hi, &guard)
            .last()
            .map(|(k, v)| (k.1, v.value.clone()))
    }

    /// Every `(version, value)` held for `key`, ascending by version.
    ///
    /// Range-scans `(key, 0)..=(key, u64::MAX)`. Tombstones are empty `Bytes`
    /// (same convention as [`Self::get`]). Lets `history_of` list versions that
    /// are committed and visible but not yet drained into `history`.
    pub fn versions_of(&self, key: &[u8]) -> Vec<(u64, Bytes)> {
        let key_rk = RecordKey::from_slice(key);
        let lo = (key_rk.clone(), 0u64);
        let hi = (key_rk, u64::MAX);

        let guard = scc::Guard::new();
        self.tree
            .range(lo..=hi, &guard)
            .map(|(k, v)| (k.1, v.value.clone()))
            .collect()
    }

    /// Remove all entries with `version <= min(durable_watermark, floor)`.
    ///
    /// `durable_watermark` — the highest version fully persisted to history.
    /// `floor` — the lowest active snapshot (no reader needs versions below).
    ///
    /// Correctness: entries above the threshold may still be read by active
    /// snapshots or by the overlay-aware read path; removing them would
    /// cause data loss.
    ///
    /// Scans the version-major range one candidate at a time.
    pub fn gc_upto(&self, durable_watermark: u64, floor: u64) {
        let threshold = durable_watermark.min(floor);
        if threshold == 0 {
            return;
        }

        self.gc_candidates(threshold, |_, _| {});
    }

    fn gc_candidates(&self, threshold: u64, mut visit: impl FnMut(u64, &RecordKey)) {
        let mut cursor: Option<(u64, RecordKey)> = None;
        loop {
            let candidate = {
                let guard = scc::Guard::new();
                let lower = cursor.clone().map_or(Unbounded, Excluded);
                let upper = threshold
                    .checked_add(1)
                    .map(|v| Excluded((v, RecordKey::from_slice(b""))))
                    .unwrap_or(Unbounded);
                self.by_version
                    .range((lower, upper), &guard)
                    .next()
                    .map(|(key, entry)| (key.clone(), Arc::clone(entry)))
            };
            let Some(((version, key), entry)) = candidate else {
                break;
            };
            visit(version, &key);
            cursor = Some((version, key.clone()));
            self.remove_if_current(&(key, version), Some(&entry));
        }
    }

    #[cfg(test)]
    pub(crate) fn gc_inspect_for_test(&self, threshold: u64, visit: impl FnMut(u64, &RecordKey)) {
        self.gc_candidates(threshold, visit);
    }

    /// Materialise the per-key winner `≤ floor` across the WHOLE overlay.
    ///
    /// Returns one entry `(key, version, value)` per distinct `key` that has
    /// at least one version `≤ floor`: the highest such version and its value
    /// (tombstone = empty `Bytes`, same convention as `get`). The caller
    /// (`current_stream` merge-join) decides tombstone suppression.
    ///
    /// The overlay is bounded by the `(durable_wm, visibility_wm]` window — it
    /// is the SMALL side of the merge, so a full lock-free iteration is
    /// acceptable. Entries are sorted by `(key, version)`, so all versions of
    /// one key are contiguous and ascending: we keep the last (highest) version
    /// `≤ floor` seen per key by overwriting as we walk the run.
    ///
    /// `floor == 0` is treated as "no version visible" → empty result, matching
    /// the bootstrap/recovery semantics of the read seams (floor 0 ⇒ no
    /// visibility restriction is applied by `current_stream`, which then never
    /// consults the overlay winner).
    pub fn snapshot_le(&self, floor: u64) -> Vec<(RecordKey, u64, Bytes)> {
        let mut out: Vec<(RecordKey, u64, Bytes)> = Vec::new();
        if floor == 0 {
            return out;
        }
        let guard = scc::Guard::new();
        // iter() yields entries in ascending (key, version) order. Within a
        // key run, the last entry with version ≤ floor is that key's winner.
        for ((k, v), val) in self.tree.iter(&guard) {
            if *v > floor {
                continue;
            }
            match out.last_mut() {
                // Same key as the previous winner — newer version ≤ floor
                // supersedes it (ascending order guarantees this is newer).
                Some((prev_key, prev_v, prev_val)) if prev_key == k => {
                    *prev_v = *v;
                    *prev_val = val.value.clone();
                }
                // New key — push its first (so-far winning) version.
                _ => out.push((k.clone(), *v, val.value.clone())),
            }
        }
        out
    }

    /// Return ALL `(key, version, value)` entries with `version <= floor`,
    /// WITHOUT collapsing per-key to the latest version (unlike
    /// [`snapshot_le`](Self::snapshot_le)).
    ///
    /// Used by the synchronous drain path
    /// ([`MvccStore::drain_to_history`](crate::mvcc_store::MvccStore::drain_to_history))
    /// which must land EVERY individual `(key, version)` pair into the durable
    /// history log — not just the per-key winner. Intermediate versions within
    /// the overlay window carry distinct commit versions, and each version is a
    /// distinct row in the version timeline.
    ///
    /// Entries are yielded in ascending `(key, version)` order (the B+ tree's
    /// natural sort), which groups all versions of one key contiguously — ideal
    /// for the drain's per-version grouping. Lock-free iteration.
    pub fn iter_all_le(&self, floor: u64) -> Vec<(RecordKey, u64, Bytes)> {
        if floor == 0 {
            return Vec::new();
        }
        let guard = scc::Guard::new();
        self.tree
            .iter(&guard)
            .filter(|((_, v), _)| *v <= floor)
            .map(|((k, v), val)| (k.clone(), *v, val.value.clone()))
            .collect()
    }

    /// Number of entries currently in the overlay.
    #[inline]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Whether the overlay is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Approximate total byte footprint (keys + values + per-entry overhead).
    ///
    /// Maintained via `AtomicUsize` for O(1) backpressure checks. The
    /// counter is advisory and may reflect concurrent mutations.
    #[inline]
    pub fn approx_bytes(&self) -> usize {
        self.byte_size.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Peek at a specific `(key, version)` entry.
    fn peek(&self, key: &[u8], version: u64) -> Option<Bytes> {
        // Inline-cheap for the 16-byte `RecordId` shape; probes the tree with
        // a `(RecordKey, version)` key.
        self.tree
            .peek_with(&(RecordKey::from_slice(key), version), |_, v| {
                v.value.clone()
            })
    }
}

impl Default for VersionedOverlay {
    fn default() -> Self {
        Self::new()
    }
}
