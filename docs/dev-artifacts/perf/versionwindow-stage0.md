בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VersionWindow Stage 0 — Probe + Design

## Verdict

**The VersionWindow unification is worth doing as architectural consolidation,
NOT as a perf win.** All three caches stay tiny in steady state (tens of
entries). O(log N) get on a tiny N is effectively O(1). The win is:

1. **Extinct bug-class**: the `commit_log_len()` O(N) full-tree walk
   (`range(..).count()`) becomes O(1) `depth.load(Relaxed)` — the same
   pattern the drainer's PT 1 already fixed. This is the only measurable
   perf improvement.
2. **Consistency**: three independent reimplementations of the same
   pattern (version-keyed scc::TreeIndex + eviction + depth tracking)
   collapse into one tested primitive.
3. **The "forgot the depth mirror" bug** (commit_write_log has no atomic
   mirror, so `commit_log_len()` is O(N)) cannot recur once the
   primitive owns the mirror.

---

## Part A — Cache Depth Probe

### Probe methodology

Two unit tests in `crates/shamir-server/src/subscriptions/tests/cache_depth_probe_tests.rs`
simulate a healthy consumer pattern:

- 1000 commits, 3 changes per commit (decode) / 3 changes x 2 modes (deliver)
- Consumer watermark advances every 5 commits (lag = 5 versions)
- Survivor count measured at each sample point

### Results

Both probes PASS with bounded assertions:

| Cache | Peak entries | Bound formula | Bound value |
|-------|-------------|---------------|-------------|
| Decode | <= 30 | `2 * lag * changes = 2*5*3` | 30 |
| Deliver | <= 60 | `2 * lag * changes * modes = 2*5*3*2` | 60 |

The depth is **deterministic from the consumer lag**: at most
`2 * consumer_lag * entries_per_commit` entries live at any point
(current batch + not-yet-evicted previous batch). Under a healthy
consumer (lag 1-2 commits), this drops to 3-6 decode entries.

### Drainer window and commit_write_log (from code reading, no new probe)

- **Drainer window** (`drainer.rs`): bounded by `(durable_watermark,
  last_committed]`. Under inline materialize, `durable_watermark ==
  last_committed` — the window is **empty** in steady state. Under
  P1d-2 (background drain), it holds the inflight tail: bounded by
  drain throughput vs commit rate. The existing overlay depth probe
  (Probe B/D) measured this at peak ~1-3 entries for InMemory and
  slightly higher for fjall. Tiny.

- **commit_write_log** (`repo_tx_gate.rs`): bounded by
  `(min_alive, latest]` where `min_alive` is the oldest open snapshot.
  Under normal operation (no long-running Serializable tx), this is
  the width of one commit batch. Even with a pinned snapshot, it grows
  linearly with commits during that snapshot's lifetime — but
  `prune_commit_log_below(min_alive)` runs on every GC tick, so it
  stays small unless a snapshot is held indefinitely (a user bug, not
  a system property). Typical steady-state: single digits.

### Conclusion

All three windows stay small (O(1) to O(tens)). An O(1)-get
specialization (e.g. backing hash-map alongside the TreeIndex) is
unnecessary. The `O(log N)` TreeIndex get on N <= 60 is effectively
constant-time (~3-4 tree levels).

---

## Part B — VersionWindow API Design

### Trait + struct

```rust
pub trait VersionKey: Ord + Clone + Send + Sync + 'static {
    /// Inclusive upper bound of all keys at `version` — for evict_through's
    /// remove_range.
    fn version_ceiling(version: u64) -> Self;
}

impl VersionKey for u64 {
    fn version_ceiling(v: u64) -> u64 { v }
}

impl VersionKey for (u64, u64, usize) {
    fn version_ceiling(v: u64) -> Self { (v, u64::MAX, usize::MAX) }
}

// deliver_cache key: (u64, u64, u64, usize, u8) — needs its own impl
impl VersionKey for (u64, u64, u64, usize, u8) {
    fn version_ceiling(v: u64) -> Self { (v, u64::MAX, u64::MAX, usize::MAX, u8::MAX) }
}

pub struct VersionWindow<K: VersionKey, V: Clone + Send + Sync + 'static> {
    tree: scc::TreeIndex<K, V>,
    depth: std::sync::atomic::AtomicUsize,
}
```

### Methods

```rust
impl<K: VersionKey, V: Clone + Send + Sync + 'static> VersionWindow<K, V> {
    /// Create an empty window.
    pub fn new() -> Self;

    /// Insert a key-value pair. Returns true on success (new key),
    /// false if the key already existed.
    /// Increments depth on success.
    pub fn insert(&self, key: K, value: V) -> bool;

    /// Lookup by exact key. O(log N).
    pub fn get(&self, key: &K) -> Option<V>;

    /// Remove a single entry by exact key. Returns true if removed.
    /// Decrements depth on success.
    pub fn remove(&self, key: &K) -> bool;

    /// Iterate over entries in [start, end] range.
    /// Returns a Vec to avoid lifetime issues with scc's Guard.
    pub fn range<R: std::ops::RangeBounds<K>>(&self, range: R) -> Vec<(K, V)>;

    /// Evict all entries with version <= `version`.
    /// Uses `K::version_ceiling(version)` to compute the upper bound.
    /// Returns the number of entries evicted (approximate — see design
    /// risk below).
    pub fn evict_through(&self, version: u64) -> usize;

    /// O(1) approximate depth. Atomic load.
    pub fn depth(&self) -> usize;
}
```

### Verification: does the API cover all three instances?

**1. Drainer window** (`K = u64, V = Arc<WalEntryV2>`):

- `offer(entry)` → `insert(entry.commit_version, entry)` + caller-side
  backpressure check via `depth() >= high_watermark`. The high-watermark
  / backpressure is drainer-specific policy wrapping `insert`, NOT part
  of VersionWindow. CLEAN.
- Contiguous-prefix scan for drain: `range(dur+1..=vis)` + caller
  iterates and breaks on gap. CLEAN.
- Gap-reseed: `seed_from_recover` = loop of `insert`. CLEAN.
- Per-entry remove after drain: `remove(&version)`. CLEAN.
- `window_depth()` for backpressure: `depth()`. CLEAN.

**2. commit_write_log** (`K = u64, V = Arc<CommitWriteRecord>`):

- `record_commit_writes(rec)` → `insert(rec.commit_version, Arc::new(rec))`.
  CLEAN.
- `predicate_conflicts(dep, snapshot)` → `range(snapshot+1..=last)` +
  iterate checking conflict. CLEAN.
- `prune_commit_log_below(floor)` → `evict_through(floor)`. CLEAN.
- `commit_log_len()` → `depth()`. O(1) instead of current O(N). CLEAN.

**3. Decode/deliver caches** (`K = (u64, u64, usize)` / `(u64, u64, u64, usize, u8)`):

- `cache_get` → `get(&key)`. CLEAN.
- `cache_insert` → `insert(key, value)`. CLEAN.
- `cache_evict_up_to(cv)` → `evict_through(cv)`. CLEAN.

**All three patterns map cleanly. No additions needed.**

Note: the caches' CAS-gated `evicted_up_to` (prevents redundant eviction
from concurrent bridges) is caller-side policy, not part of
VersionWindow — the caller wraps `evict_through` in its own
`AtomicU64::compare_exchange` gate, just as today.

### Design risk: `evict_through` depth accuracy

**Problem:** `scc::TreeIndex::remove_range` does NOT return a count of
removed entries. To keep `depth` accurate, `evict_through` must either:

**(a) Two-pass: count then remove.** Walk `range(..=ceiling)` to count N,
then `remove_range(..=ceiling)`, then `depth.fetch_sub(N)`. Cost: O(N)
extra pass over the evicted range.

**(b) Approximate depth.** Just call `remove_range`, don't adjust depth.
Depth drifts until the next full-tree audit (e.g. periodic
`tree.iter().count()`). Self-correcting but lossy.

**Recommendation: (a) two-pass exact count.** Rationale:

- The evicted range is tiny (Part A proved all windows stay small).
  An extra O(evicted) pass over 10-30 entries is ~nanoseconds.
- The drainer's existing `window_depth` is already exact (it adjusts on
  every insert/remove individually). An approximate depth would be a
  regression from the drainer's current precision.
- `commit_log_len()` is consumed as telemetry — an inaccurate value
  would make the metric useless.
- Stage 2/3's /opti before-after bench MUST confirm this extra pass does
  not regress eviction throughput. Given N <= 60, this is virtually
  guaranteed.

**Alternative considered:** per-entry `remove` in a loop (count as you
go). This avoids the double-scan but replaces one `remove_range` (which
scc optimizes internally) with N individual `remove` calls. For N <= 60,
the difference is negligible; `remove_range` + count is cleaner code.

---

## Part C — Home Crate

**Decision: `shamir-collections`** is the right home.

`shamir-collections` (`crates/shamir-collections/Cargo.toml`) currently
depends only on `indexmap` + `fxhash`. Adding `scc` is a new dependency
but a clean one:

- `scc` is already in the workspace (used by `shamir-tx`, `shamir-engine`,
  `shamir-server`). No new external dependency.
- `shamir-collections` is a leaf crate — depended on by `shamir-tx`,
  `shamir-engine`, `shamir-server` (all three consumers of VersionWindow).
  It does NOT depend on any of them. No cycle risk.
- It already defines `THasher` and `TMap`/`TSet` — collection primitives
  are its charter.

The only consideration: `shamir-collections` is described as "WASM-friendly
leaf crate." Adding `scc` (which uses `std::sync::atomic`) is fine — `scc`
compiles to wasm32-unknown-unknown (atomics are supported via
`wasm32-unknown-unknown` with `atomics` target feature). But if strict
no-std WASM compatibility is needed, `VersionWindow` could be feature-gated
behind `#[cfg(feature = "concurrent")]`. This is a Stage 1 decision.

---

## Part D — /opti Baseline Inventory

### Stage 2 (cache migration): `cache_struct_tradeoff` bench

Bench: `crates/shamir-server/benches/cache_struct_tradeoff.rs`

**Baseline snapshot (2026-06-22, QUICK tier):**

| Group | Variant | N | Time | Throughput |
|-------|---------|---|------|------------|
| cache_get | dashmap O(1) | 1K | 336 ns | 2.97 Melem/s |
| cache_get | treeindex O(log N) | 1K | 1.04 us | 961 Kelem/s |
| cache_get | dashmap O(1) | 10K | 484 ns | 2.06 Melem/s |
| cache_get | treeindex O(log N) | 10K | 1.25 us | 802 Kelem/s |
| cache_get | dashmap O(1) | 50K | 603 ns | 1.66 Melem/s |
| cache_get | treeindex O(log N) | 50K | 1.34 us | 745 Kelem/s |
| cache_evict_half | dashmap retain O(cache) | 1K | 228 us | 2.19 Melem/s |
| cache_evict_half | treeindex remove_range | 1K | 11.3 us | 44.4 Melem/s |
| cache_evict_half | dashmap retain O(cache) | 10K | 2.48 ms | 2.01 Melem/s |
| cache_evict_half | treeindex remove_range | 10K | 20.3 us | 246 Melem/s |
| cache_evict_half | dashmap retain O(cache) | 50K | 18.4 ms | 1.36 Melem/s |
| cache_evict_half | treeindex remove_range | 50K | 38.4 us | 651 Melem/s |

The TreeIndex numbers are the Stage 2 "before" snapshot. After wrapping
in VersionWindow, the get/evict costs must not regress beyond noise.

### Stage 3 (commit_write_log migration)

No dedicated bench exists yet. Stage 3 must write a conflict-scan +
eviction bench (`crates/shamir-tx/benches/commit_log_bench.rs` or
similar) as its first step, capture baseline on the raw
`scc::TreeIndex` code, then migrate and re-measure.

### Stage 4 (drainer window, DEFERRED)

Bench: `crates/shamir-engine/benches/drain_cost_vs_depth.rs` (exists).
Stage 4 is deferred — the drainer is the most complex consumer and
carries the most risk. Run the existing bench as-is for a baseline
when/if Stage 4 is sanctioned.

---

## Exclusion: VersionedOverlay

`VersionedOverlay` (`crates/shamir-tx/src/versioned_overlay.rs`) is
**explicitly excluded** from VersionWindow. Its key is `(Bytes key, u64
version)` — version is SECONDARY (the second tuple component). It needs:

- **Key-major range reads**: `newest_visible(key, vis)` scans
  `(key, 0)..=(key, vis)` to find the latest version for a specific key.
- **Version-major eviction**: `gc_upto(watermark)` removes all entries
  with `version <= watermark` — but since version is the SECOND
  component, this is an O(total) full-tree scan.

This is a genuinely different (harder) shape from VersionWindow, which
assumes version is the PRIMARY (first) key component enabling O(evicted)
range eviction.

A prior campaign measured the overlay's O(N) `gc_upto` cliff and found it
**never bites in practice**: the drainer keeps the overlay at 1-3 entries
in steady state (Probe A/B/D in `overlay_depth_probe.rs`), so the full
scan touches 1-3 nodes. Optimizing it would be negative ROI. The overlay
stays as-is.
