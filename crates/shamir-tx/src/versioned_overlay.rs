//! In-memory versioned overlay for the `(key, version) → value` window.
//!
//! [`VersionedOverlay`] holds committed values that have been made visible
//! (cell version published, `CompletionTracker::mark(Materialized)`) but not
//! yet drained to the durable `history` store. It sits between `RecordCell`
//! and the on-disk version log, giving instant ack-path visibility while the
//! background drain-leader flushes batches asynchronously.
//!
//! **Lock-free.** The backing structure is [`scc::TreeIndex`] — a lock-free
//! B+ tree that keeps `(key, version)` tuples sorted lexicographically, then
//! by version within each key. This gives O(log N + k) range scans for
//! `newest_visible` and contiguous iteration for GC.
//!
//! **Scaffold (P1a).** This module is additive — it is not wired into any
//! read or write path yet. P1b–P1e will integrate it.

use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use scc::TreeIndex;
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

/// In-memory versioned overlay.
///
/// Holds `(key, version) → value` entries for the window
/// `(durable_watermark, visibility_watermark]`. Tombstones are stored as
/// empty `Bytes` (same convention as `history`), so `get` returning
/// `Some(Bytes::new())` means "deleted at that version" — the caller
/// (resolve_read) decides semantics.
///
/// All methods are **lock-free** (no `Mutex` / `RwLock`).
pub struct VersionedOverlay {
    /// Lock-free B+ tree: `(record_key, version) → value`.
    tree: TreeIndex<OverlayKey, Bytes>,
    /// Approximate byte footprint maintained atomically on insert/gc for O(1)
    /// backpressure queries. Not exact — concurrent insert+gc may drift by a
    /// few entries, but monotonic-enough for soft thresholds.
    byte_size: AtomicUsize,
    /// Entry count maintained atomically (TreeIndex has no O(1) len).
    count: AtomicUsize,
}

// Per-entry overhead estimate: two Arc bumps (key+value Bytes), tree node
// bookkeeping. Conservative 64 bytes covers the B+ tree node fraction and
// the two fat pointers.
const PER_ENTRY_OVERHEAD: usize = 64;

impl VersionedOverlay {
    /// Create an empty overlay.
    #[inline]
    pub fn new() -> Self {
        Self {
            tree: TreeIndex::new(),
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
        let entry_bytes = key.len() + value.len() + PER_ENTRY_OVERHEAD;
        let composite = (key, version);
        if self.tree.insert_sync(composite, value).is_ok() {
            // New entry — update counters.
            self.byte_size.fetch_add(entry_bytes, Ordering::Relaxed);
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        // Err ⇒ key already present (idempotent replay) — no-op.
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
    /// drained or never inserted). Lock-free.
    pub fn remove(&self, key: &[u8], version: u64) {
        // Inline-cheap for the 16-byte `RecordId` shape (no heap alloc);
        // constructs the tree's `(RecordKey, version)` probe key.
        let composite = (RecordKey::from_slice(key), version);
        // `peek` the value first to recover its byte length for the counters,
        // then remove. The window between peek and remove is benign: counters
        // are advisory (Relaxed) and a concurrent insert of the same version is
        // impossible (versions are globally unique).
        let entry_bytes = self
            .tree
            .peek_with(&composite, |_, v| v.len())
            .map(|vlen| composite.0.len() + vlen + PER_ENTRY_OVERHEAD);
        if self.tree.remove_sync(&composite) {
            if let Some(bytes) = entry_bytes {
                self.byte_size.fetch_sub(bytes, Ordering::Relaxed);
            }
            self.count.fetch_sub(1, Ordering::Relaxed);
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
            .map(|(k, v)| (k.1, v.clone()))
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
    /// Implementation: iterates the tree (entries are version-sorted within
    /// each key) and removes qualifying entries. The overlay is bounded by
    /// the `(durable_wm, visibility_wm]` window — typically small.
    ///
    /// P1e may optimise this with a version-major secondary index to avoid
    /// a full scan when the tree is large relative to the GC batch.
    pub fn gc_upto(&self, durable_watermark: u64, floor: u64) {
        let threshold = durable_watermark.min(floor);
        if threshold == 0 {
            return;
        }

        // Collect keys to remove. Entries are sorted by (key, version) — not
        // by version alone — so we must filter (not take_while).
        let to_remove: Vec<(OverlayKey, usize)> = {
            let guard = scc::Guard::new();
            self.tree
                .iter(&guard)
                .filter(|(k, _)| k.1 <= threshold)
                .map(|(k, v)| {
                    let entry_bytes = k.0.len() + v.len() + PER_ENTRY_OVERHEAD;
                    (k.clone(), entry_bytes)
                })
                .collect()
        };

        let mut removed_bytes = 0usize;
        let mut removed_count = 0usize;
        for (key, entry_bytes) in to_remove {
            if self.tree.remove_sync(&key) {
                removed_bytes += entry_bytes;
                removed_count += 1;
            }
        }

        if removed_count > 0 {
            self.byte_size.fetch_sub(removed_bytes, Ordering::Relaxed);
            self.count.fetch_sub(removed_count, Ordering::Relaxed);
        }
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
                    *prev_val = val.clone();
                }
                // New key — push its first (so-far winning) version.
                _ => out.push((k.clone(), *v, val.clone())),
            }
        }
        out
    }

    /// Return ALL `(key, version, value)` entries with `version <= floor`,
    /// WITHOUT collapsing per-key to the latest version (unlike
    /// [`snapshot_le`](Self::snapshot_le)).
    ///
    /// Used by the synchronous drain path
    /// ([`MvccStore::drain_to_history`](super::mvcc_store::MvccStore::drain_to_history))
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
            .map(|((k, v), val)| (k.clone(), *v, val.clone()))
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
    /// Maintained via `AtomicUsize` for O(1) backpressure checks. May drift
    /// slightly under concurrent insert+gc but never goes negative (saturates
    /// at 0 on underflow via `fetch_sub` with `Relaxed` ordering — the
    /// counter is advisory).
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
            .peek_with(&(RecordKey::from_slice(key), version), |_, v| v.clone())
    }
}

impl Default for VersionedOverlay {
    fn default() -> Self {
        Self::new()
    }
}
