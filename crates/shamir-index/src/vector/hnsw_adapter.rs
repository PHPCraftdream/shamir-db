//! HNSW approximate nearest neighbor adapter using `hnsw_rs`.
//!
//! `hnsw_rs::Hnsw` is internally thread-safe (RwLock per layer for
//! insert, lock-free traversal for search). We wrap it directly —
//! no actor needed for HNSW itself.
//!
//! Deletion via soft-delete tombstone set; search over-scans ×2 to
//! compensate for filtered-out tombstones.

use super::adapter::{SearchOpts, VectorAdapter, VectorError};
use super::quantized_dist::{rescore_f32, ShamirDistU8};
use super::simd::{dot_product, l2_squared};
use super::sq8::Sq8Quantizer;
use crate::kind::{VectorMetric, VectorQuantization};
use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use hnsw_rs::anndists::dist::distances::Distance;
use hnsw_rs::hnsw::Hnsw;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Maximum allowed top-k value. Untrusted `k` near `u32::MAX` would drive
/// `overscan*2+10` and `Vec::with_capacity(k+16)` to multi-GB allocation.
///
/// P3 / V3.1: `pub` so the engine-side filtered-ANN retry loop
/// (`read_filtered_vector_scan`) can clamp its widening `k′ = k × oversample`
/// against the same bound the adapter enforces internally. Without a shared
/// cap the retry could request `k′` far above what the adapter accepts, the
/// adapter would silently truncate, and the retry would loop forever
/// (post-filter always < k because the adapter never returned k′ candidates).
pub const MAX_TOPK: u32 = 10_000;

/// Maximum allowed per-query `ef_search` value. Untrusted `ef` near
/// `u32::MAX` would drive `hnsw.search(query, overscan, ef)` to explore an
/// enormous graph fan-out (CPU-bound `spawn_blocking` holding the rayon pool).
/// Clamped (NOT rejected) at this cap — a huge `ef` behaves identically to
/// `MAX_EF_SEARCH` for recall but cannot starve the worker pool.
///
/// 10_000 matches `MAX_TOPK`: `ef >= k` is the standard HNSW guidance, so
/// capping `ef` at the same bound as `k` keeps the knobs consistent. Real
/// recall gains plateau well below this (typical sweet spots: 50–500).
pub const MAX_EF_SEARCH: u32 = 10_000;

/// Live-element count at or below which `search` runs an EXACT brute-force
/// scan instead of the approximate HNSW graph.
///
/// `hnsw_rs` 0.3.x assigns node layers from an internal, **unseedable** RNG,
/// so a freshly-built graph over a tiny dataset is nondeterministic: recall
/// can drop below 100% and the same query can return different neighbours
/// across builds (and across reopen). On a handful of points that surfaces as
/// flaky / wrong top-k. Brute-force over a few hundred vectors is microseconds
/// and GUARANTEES exact, stable results; HNSW only earns its keep at larger N
/// where the graph is well-connected and recall is reliable. 256 keeps small
/// indexes (and the exact-assertion tests) deterministic while leaving the
/// recall-tolerance tests (≥1k vectors) on the HNSW path.
const BRUTE_FORCE_MAX: usize = 256;

// #424 (Б-4) — test-only hook for deterministically reproducing the
// transient-None race window in `search`/`search_cofilter`.
//
// The race is between `quantized_active()` (Acquire load) and
// `hnsw.load_full()` (ArcSwap load) — two atomic reads with NO `.await`
// between them in the f32-graph branch. Under normal execution the window
// is a few nanoseconds of synchronous code; a statistical test cannot
// reliably hit it (confirmed: 3× consecutive runs of a tight-loop
// concurrent test passed even with the fix disabled).
//
// This hook lets a test PAUSE a search request at exactly the point where
// the race lives — AFTER `quantized_active() == false` has been read (so
// the request is committed to the f32 branch) but BEFORE
// `hnsw.load_full()`. The test then triggers a fit transition (which
// drops the f32 graph), confirms the drop, and releases the paused
// request — which now observes `load_full() == None` and must exercise the
// retry path.
//
// The hook carries TWO pieces of state:
//  * `arrived` — set by the search request when it REACHES the gate (so
//    the test knows the request is past the `quantized_active()` check and
//    is now paused inside the f32 branch).
//  * `notify` — the pause/release mechanism. The search request awaits
//    `notify.notified()`; the test calls `notify.notify_one()` to release
//    it after confirming the f32 graph has been dropped.
//
// `ArcSwapOption` so tests can install/clear it freely. Compiles away
// entirely in release builds (`#[cfg(test)]`).
#[cfg(test)]
pub(crate) struct TestF32Gate {
    pub(crate) arrived: std::sync::atomic::AtomicBool,
    pub(crate) notify: tokio::sync::Notify,
}

#[cfg(test)]
static TEST_SEARCH_F32_GATE: std::sync::LazyLock<arc_swap::ArcSwapOption<TestF32Gate>> =
    std::sync::LazyLock::new(arc_swap::ArcSwapOption::empty);

/// #424 (Б-4) — install the test-only f32-gate hook. Returns the `Arc`
/// so the test can: (a) poll `.arrived` to confirm the search request has
/// reached the gate, and (b) call `.notify.notify_one()` to release it
/// after confirming the f32 graph has been dropped. Test-only.
#[cfg(test)]
pub(crate) fn install_test_search_f32_gate() -> Arc<TestF32Gate> {
    let gate = Arc::new(TestF32Gate {
        arrived: std::sync::atomic::AtomicBool::new(false),
        notify: tokio::sync::Notify::new(),
    });
    TEST_SEARCH_F32_GATE.store(Some(Arc::clone(&gate)));
    gate
}

/// #424 (Б-4) — clear the test-only f32-gate hook (between tests).
#[cfg(test)]
pub(crate) fn clear_test_search_f32_gate() {
    TEST_SEARCH_F32_GATE.store(None);
}

#[derive(Debug, Clone, Copy)]
pub struct ShamirDist {
    pub(crate) metric: VectorMetric,
}

impl Distance<f32> for ShamirDist {
    fn eval(&self, a: &[f32], b: &[f32]) -> f32 {
        // Route through the shared SIMD kernels (AVX2+FMA when available,
        // chunked-scalar fallback). `hnsw_rs` calls `eval` for every
        // distance computation during graph traversal and insertion —
        // this is the production hot path. Semantics are preserved
        // bit-for-bit-modulo-FMA-rounding (kernels match the original
        // sum/zip semantics; FMA differs by at most 0.5 ulp per op,
        // within existing test tolerances).
        match self.metric {
            VectorMetric::L2 => l2_squared(a, b).sqrt(),
            VectorMetric::Cosine => {
                let dot = dot_product(a, b);
                let na = dot_product(a, a).sqrt();
                let nb = dot_product(b, b).sqrt();
                if na < 1e-9 || nb < 1e-9 {
                    return 1.0;
                }
                // `.max(0.0)`: Cauchy-Schwarz gives `dot <= na*nb` so the
                // distance is >= 0 mathematically, but f32/FMA rounding in the
                // separately-computed `dot` and `na*nb` can yield a tiny
                // negative — which trips hnsw_rs's strict non-negative-distance
                // assertion. Clamping only ever corrects that rounding artifact
                // (a negative cosine distance is never legitimate); all valid
                // values pass through unchanged. Mirrors the `Dot` arm below.
                (1.0 - dot / (na * nb)).max(0.0)
            }
            VectorMetric::Dot => {
                // HNSW requires non-negative distances. For normalized
                // vectors, dot ∈ [-1, 1] and dist = 1 - dot ∈ [0, 2]
                // preserves the search ordering. Callers must normalize
                // their vectors for correct top-k with `Dot`.
                let dot = dot_product(a, b);
                (1.0 - dot).max(0.0)
            }
        }
    }
}

#[derive(Clone)]
pub struct HnswConfig {
    pub max_elements: usize,
    pub m: usize,
    pub max_layer: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            max_elements: 10_000,
            m: 16,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 50,
        }
    }
}

pub struct HnswAdapter {
    dim: u32,
    metric: VectorMetric,
    ef_search: usize,
    /// Opt-in quantization mode. `None` → the adapter never builds a u8
    /// graph and the entire pipeline is the legacy f32 path, bit-for-bit.
    quantization: Option<VectorQuantization>,
    /// The legacy f32 graph. Carried in an [`ArcSwapOption`] (NOT a plain
    /// `Arc`) so a quantized adapter can **drop** it after fit, freeing the
    /// ~4·dim·N bytes of f32 vectors that `hnsw_rs` retains internally —
    /// this is the whole point of SQ8 quantization (#418).
    ///
    /// # Lifetimes / `Option`
    /// - **Unquantized adapter** (`quantization == None`): the f32 graph is
    ///   installed at construction and NEVER cleared — every code path is
    ///   the legacy f32 pipeline, and [`Self::hnsw_load`] returns `Some`
    ///   for the lifetime of the adapter.
    /// - **Quantized adapter, pre-fit**: installed at construction; the f32
    ///   buffer + graph are the active search path.
    /// - **Quantized adapter, post-fit**: cleared via [`Self::drop_f32_graph`]
    ///   once the u8 graph + codes + catch-up drain are fully published. At
    ///   that point every search/upsert is gated through `quantized_active()`
    ///   and the f32 graph is unobservable. All f32-path call sites are
    ///   reached only when `!quantized_active()`, so `hnsw_load()` returning
    ///   `None` there is an invariant violation, surfaced as an error (NOT a
    ///   panic on the normal path) — see each site's justification.
    ///
    /// # RCU drop — no UAF
    ///
    /// `store(None)` is a Release store; in-flight readers that already did
    /// `load_full()` hold their own `Arc` clone and finish their graph
    /// traversal against the now-private copy. `Arc::strong_count` reaching 0
    /// frees the graph after the last reader drops its clone. No reader
    /// observes a dangling pointer. Mirrors the same lock-free RCU pattern
    /// already used for `hnsw_u8` (the load counterpart of this same slot).
    hnsw: ArcSwapOption<Hnsw<'static, f32, ShamirDist>>,
    pub(crate) rid_map: scc::HashMap<usize, RecordId, THasher>,
    rid_to_internal: scc::HashMap<RecordId, usize, THasher>,
    /// Raw vectors retained (keyed by internal id) so a small index can be
    /// searched EXACTLY by brute force — see [`BRUTE_FORCE_MAX`]. Tombstoned
    /// entries are removed here on replace/delete.
    ///
    /// In the quantized path this map holds f32 vectors ONLY pre-fit; once
    /// [`Self::is_fitted`] flips true the post-fit inserts go to
    /// [`Self::vectors_u8`] and this map is drained.
    vectors: scc::HashMap<usize, Vec<f32>, THasher>,
    pub(crate) deleted: scc::HashMap<usize, (), THasher>,
    deleted_count: AtomicUsize,
    next_id: AtomicUsize,
    /// V4.2 (#408) — tombstone set keyed by RecordId, present ONLY on a
    /// compaction-target adapter (Some during compaction, None otherwise).
    /// Populated by double-write deletes; consulted by `backfill_if_absent`
    /// and Step4b reconcile to prevent ghost resurrection.
    pub(crate) compaction_deleted_rids: Option<Arc<scc::HashMap<RecordId, (), THasher>>>,

    // === V5.2 (#411) — quantized (SQ8) u8-graph path ===
    //
    // All of these stay `None`/empty when `quantization` is `None` (the
    // legacy f32 path). When `quantization == Some(Sq8)`, the adapter runs
    // f32-only below [`FIT_THRESHOLD`] (256, == [`BRUTE_FORCE_MAX`]) and
    // crosses to the u8 graph at the first upsert that reaches the
    // threshold — see [`Self::try_fit_and_rebuild`].
    //
    // Concurrency of the fit transition: a single-flight guard
    // (`fit_in_flight`) ensures exactly ONE caller performs the rebuild;
    // concurrent upserts that arrive during the rebuild continue to append
    // to the f32 buffer, and after the atomic swap the fitter re-inserts
    // any internals that were added after it snapshotted the buffer (the
    // "delta re-insert" strategy from #408). No mutation is lost.
    //
    // `is_fitted` is the publish flag: once `true`, every search and every
    // insert goes through the u8 graph. Relaxed `is_fitted` loads are
    // sufficient because the `ArcSwapOption` store of `hnsw_u8` provides the
    // happens-before edge (Release on store, Acquire on `load_full`). We use
    // `ArcSwapOption` — NOT `std::sync::Mutex` — because although the WRITE
    // (fit) is a one-shot low-frequency transition, the READ
    // (`hnsw_u8_handle` on every quantized search) is a hot path: a mutex
    // there would serialise all concurrent quantized searches. `load_full`
    // is wait-free (an Arc clone, never a Guard held across `.await`).
    hnsw_u8: arc_swap::ArcSwapOption<Hnsw<'static, u8, ShamirDistU8>>,
    /// SQ8 codes keyed by internal id. Populated at fit and on every
    /// post-fit insert. The u8 graph ALSO stores its own `Vec<u8>` per
    /// DataPoint, so this map is technically redundant with the graph's
    /// internal copy — BUT the graph does not expose a `get_vector(id)`
    /// accessor, so we keep the authoritative codes here for rescore and
    /// `collect_live_vectors`. This is the 4×-memory win: u8 codes (dim
    /// bytes) vs f32 vectors (4·dim bytes).
    vectors_u8: scc::HashMap<usize, Vec<u8>, THasher>,
    /// Frozen quantizer params. `None` until fit; `Some(Arc::new(q))`
    /// after fit (read-only for the lifetime of the adapter).
    quantizer: std::sync::OnceLock<Arc<Sq8Quantizer>>,
    /// Publish flag: `true` once the u8 graph is live. See the struct doc
    /// for the memory-ordering argument.
    is_fitted: AtomicBool,
    /// Single-flight guard: the FIRST caller to CAS this `true` performs
    /// the fit+rebuild; every other concurrent caller skips straight to
    /// the f32 path (their internals are re-inserted by the fitter's
    /// delta pass after the swap).
    fit_in_flight: AtomicBool,
    /// #423 (Б-3) — count of pre-flip internals (`internal <
    /// next_id_at_flip`) that have been ACCOUNTED FOR — either landed in
    /// `vectors_u8` (claimed) or tombstoned. The fit catch-up loop uses
    /// this for an EXACT O(1) convergence check that cannot be inflated by
    /// post-flip upserts (which allocate internals `>= next_id_at_flip`
    /// and never migrate through the f32 buffer).
    ///
    /// Incremented for a given `internal < next_id_at_flip` under exactly
    /// ONE of two conditions (mutually exclusive — a tombstoned internal
    /// is filtered out of every claim path by `deleted.contains`, so it
    /// can never ALSO win a claim):
    ///  - a `vectors_u8` insert returns `Ok` (the atomic claim won — first
    ///    time this `internal` enters `vectors_u8`), via
    ///    [`Self::claim_and_publish_u8`]/`claim_and_publish_u8_async`; or
    ///  - a `deleted` insert returns `Ok` (the atomic tombstone won) at one
    ///    of the three tombstone call sites (`upsert`, `upsert_batch`,
    ///    `delete`).
    ///
    /// Because both `scc::HashMap::insert` calls return `Ok` exactly once
    /// per key, the counter is incremented exactly once per distinct
    /// pre-flip internal — no double-counting, no missed increments.
    /// Readers (the fit loop) compare it against `next_id_at_flip`; the
    /// loop breaks when equality holds. See the seed comment in
    /// `try_fit_and_rebuild` for why a previous version of this fix (which
    /// subtracted a frozen `deleted_count_at_flip` from the target instead
    /// of folding tombstones into this counter) could hang forever on a
    /// post-flip tombstone of a pre-flip internal.
    migrated_pre_flip: AtomicUsize,
    /// #423 (Б-3) — the value of `next_id` at the moment `is_fitted`
    /// flipped true. `usize::MAX` sentinel = "not fitted / no flip
    /// boundary" (the value is inert before the first fit). Both the
    /// fit catch-up loop AND the upsert self-migration path read this to
    /// decide whether a winning `vectors_u8` claim should bump
    /// `migrated_pre_flip` (only internals `< next_id_at_flip` count —
    /// they are the pre-flip internals the catch-up loop waits for).
    /// Written ONCE by the fitter (right before the flip), read
    /// concurrently thereafter; `AtomicUsize` keeps the read O(1) and
    /// lock-free.
    next_id_at_flip: AtomicUsize,
}

/// At or above this live-element count a `quantization == Some(Sq8)`
/// adapter fits the SQ8 quantizer and builds the u8 graph. Deliberately
/// equal to [`BRUTE_FORCE_MAX`]: below the threshold the exact brute-force
/// path over f32 vectors is cheaper than a graph traversal AND 100%
/// recall, so quantizing earlier would only lose accuracy for no gain.
const FIT_THRESHOLD: usize = BRUTE_FORCE_MAX;

impl HnswAdapter {
    pub fn new(dim: u32, metric: VectorMetric, config: HnswConfig) -> Self {
        Self::new_with_quantization(dim, metric, config, None)
    }

    /// V5.2 (#411) — construct an adapter with an opt-in quantization mode.
    ///
    /// `quantization == None` is bit-for-bit identical to [`Self::new`] —
    /// the u8-graph fields stay `None`/empty forever and every code path
    /// is the legacy f32 pipeline.
    ///
    /// `quantization == Some(Sq8)` enables deferred SQ8 quantization: the
    /// adapter runs f32 below [`FIT_THRESHOLD`] and crosses to a u8 graph
    /// at the threshold — see [`Self::try_fit_and_rebuild`] and the struct
    /// doc on `HnswAdapter`.
    pub fn new_with_quantization(
        dim: u32,
        metric: VectorMetric,
        config: HnswConfig,
        quantization: Option<VectorQuantization>,
    ) -> Self {
        let dist = ShamirDist { metric };
        let hnsw = Hnsw::new(
            config.m,
            config.max_elements,
            config.max_layer,
            config.ef_construction,
            dist,
        );
        let cap = config.max_elements;
        Self {
            dim,
            metric,
            ef_search: config.ef_search,
            quantization,
            hnsw: ArcSwapOption::new(Some(Arc::new(hnsw))),
            rid_map: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            rid_to_internal: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            vectors: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            deleted: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            deleted_count: AtomicUsize::new(0),
            next_id: AtomicUsize::new(0),
            compaction_deleted_rids: None,
            hnsw_u8: arc_swap::ArcSwapOption::empty(),
            vectors_u8: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            quantizer: std::sync::OnceLock::new(),
            is_fitted: AtomicBool::new(false),
            fit_in_flight: AtomicBool::new(false),
            migrated_pre_flip: AtomicUsize::new(0),
            next_id_at_flip: AtomicUsize::new(usize::MAX),
        }
    }

    // ----------------------------------------------------------------------
    // Snapshot codec accessors (`pub(crate)` — used by `snapshot.rs` only)
    // ----------------------------------------------------------------------
    //
    // The codec needs read access to the adapter's internal maps + Hnsw handle
    // to serialise a snapshot, and write access (`from_parts`) to rebuild one
    // from a loaded graph. Both are `pub(crate)`: the codec lives in the same
    // crate, and there is no reason for an external caller to touch these.

    pub(crate) fn dim_field(&self) -> u32 {
        self.dim
    }

    pub(crate) fn metric_field(&self) -> VectorMetric {
        self.metric
    }

    pub(crate) fn ef_search_field(&self) -> usize {
        self.ef_search
    }

    /// #418 — RCU load of the f32 graph: returns a private `Arc` clone (or
    /// `None` if a quantized adapter has dropped it post-fit). Callers on the
    /// f32 search/insert path do `load_full()` once and hold the Arc for the
    /// duration of the graph op; the Arc keeps the graph alive even if a
    /// concurrent fit does `store(None)` — RCU, no UAF (see the `hnsw` field
    /// doc).
    pub(crate) fn hnsw_load(&self) -> Option<Arc<Hnsw<'static, f32, ShamirDist>>> {
        self.hnsw.load_full()
    }

    /// #418 — `true` iff the f32 graph is currently resident. Used by the
    /// memory-regression test to deterministically assert that a fitted
    /// quantized adapter HAS dropped its f32 graph (and an unquantized /
    /// pre-fit adapter has NOT). This is a stronger, deterministic check than
    /// a flaky RSS sample — see task #418.
    #[allow(dead_code)] // API for #418 memory-regression test
    pub(crate) fn f32_graph_present(&self) -> bool {
        self.hnsw.load_full().is_some()
    }

    /// Build parameters for the snapshot sidecar. Prefers the f32 graph (the
    /// legacy source), falls back to the u8 graph when the f32 graph has been
    /// dropped post-fit (#418) — `m`/`max_layer`/`ef_construction` are
    /// identical across the two graphs (both built from the same `HnswConfig`
    /// at fit time).
    ///
    /// Returns `(m: u8, max_layer: usize, ef_construction: usize)` matching
    /// the `SnapshotSidecar` field types. `None` only if BOTH graphs are
    /// absent — an invariant violation (an adapter always has at least one
    /// graph); callers treat `None` as a snapshot error.
    pub(crate) fn build_params(&self) -> Option<(u8, usize, usize)> {
        if let Some(h) = self.hnsw.load_full() {
            return Some((
                h.get_max_nb_connection(),
                h.get_max_level(),
                h.get_ef_construction(),
            ));
        }
        if let Some(h) = self.hnsw_u8.load_full() {
            return Some((
                h.get_max_nb_connection(),
                h.get_max_level(),
                h.get_ef_construction(),
            ));
        }
        None
    }

    pub(crate) fn next_id_value(&self) -> usize {
        self.next_id.load(Ordering::Relaxed)
    }

    /// Iterate `(internal -> rid)` pairs for snapshot serialisation. Borrows
    /// each entry read-only under the scc cursor; the closure must not block.
    pub(crate) fn for_each_rid_map<F: FnMut(usize, RecordId)>(&self, mut f: F) {
        self.rid_map.scan(|internal, rid| {
            f(*internal, *rid);
        });
    }

    /// Iterate `(rid -> internal)` pairs for snapshot serialisation.
    pub(crate) fn for_each_rid_to_internal<F: FnMut(RecordId, usize)>(&self, mut f: F) {
        self.rid_to_internal.scan(|rid, internal| {
            f(*rid, *internal);
        });
    }

    /// Check if a rid exists in the live index (not tombstoned).
    #[allow(dead_code)] // API for #408 compaction tests
    pub(crate) fn contains_rid(&self, rid: &RecordId) -> bool {
        self.rid_to_internal.contains(rid)
    }

    /// Iterate the tombstone (`deleted`) internals for snapshot serialisation.
    pub(crate) fn for_each_deleted<F: FnMut(usize)>(&self, mut f: F) {
        self.deleted.scan(|internal, ()| {
            f(*internal);
        });
    }

    /// Iterate `(internal -> vector)` pairs for snapshot serialisation.
    pub(crate) fn for_each_vector<F: FnMut(usize, &[f32])>(&self, mut f: F) {
        self.vectors.scan(|internal, vec| {
            f(*internal, vec);
        });
    }

    /// Reconstruct an adapter from snapshot parts. Used by `snapshot::load`.
    ///
    /// `hnsw` is an `Arc<Hnsw<'static, ...>>` obtained from `load_hnsw_with_dist`
    /// via a `Box::leak`'d `HnswIo` loader (see `snapshot::load` — the leak is
    /// boot-only, one loader per shard, and the dump files are the durable
    /// artefact). The maps and `next_id` are rebuilt from the sidecar.
    ///
    /// V5.2 (#411): snapshot codec always loads the f32 path here — the
    /// quantized snapshot (u8 graph + quantizer params) is #412. A
    /// quantization-enabled adapter loaded this way starts un-fitted and
    /// will re-fit at the threshold on the next upserts, which is correct
    /// (the f32 vectors are all present in the sidecar).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        dim: u32,
        metric: VectorMetric,
        ef_search: usize,
        hnsw: Arc<Hnsw<'static, f32, ShamirDist>>,
        rid_map: scc::HashMap<usize, RecordId, THasher>,
        rid_to_internal: scc::HashMap<RecordId, usize, THasher>,
        vectors: scc::HashMap<usize, Vec<f32>, THasher>,
        deleted: scc::HashMap<usize, (), THasher>,
        next_id: usize,
    ) -> Self {
        #[allow(clippy::disallowed_methods)] // O(N) ack: one-time seed at snapshot load
        let deleted_cnt = deleted.len();
        Self {
            dim,
            metric,
            ef_search,
            quantization: None,
            hnsw: ArcSwapOption::new(Some(hnsw)),
            rid_map,
            rid_to_internal,
            vectors,
            deleted,
            deleted_count: AtomicUsize::new(deleted_cnt),
            next_id: AtomicUsize::new(next_id),
            compaction_deleted_rids: None,
            hnsw_u8: arc_swap::ArcSwapOption::empty(),
            vectors_u8: scc::HashMap::with_hasher(THasher::default()),
            quantizer: std::sync::OnceLock::new(),
            is_fitted: AtomicBool::new(false),
            fit_in_flight: AtomicBool::new(false),
            // Unfitted on load (v1 path) — no pre-flip migration has run.
            migrated_pre_flip: AtomicUsize::new(0),
            next_id_at_flip: AtomicUsize::new(usize::MAX),
        }
    }

    /// V5.3 (#412) — Reconstruct a FITTED quantized adapter from snapshot
    /// parts. Used by `snapshot::load` when the sidecar carries a
    /// `quantization = Some(bincode(QuantMeta))` field and the manifest
    /// points at `qgraph`/`qdata` sections (a quantized v2 snapshot).
    ///
    /// This is the load counterpart of the dump branch in
    /// [`dump_snapshot_with_gen`](crate::vector::snapshot::dump_snapshot_with_gen):
    /// it stitches the already-loaded u8 graph (`hnsw_u8`), the reconstructed
    /// [`Sq8Quantizer`] (`quantizer`), and the sidecar's `vectors_u8` codes
    /// into a live adapter with `is_fitted = true`. Post-load search and
    /// upsert go through the quantized path immediately — no re-fit is
    /// needed.
    ///
    /// `quantization` is set to `Some(Sq8)` so [`quantized_active`](Self::quantized_active)
    /// returns `true` (it gates on `is_fitted && quantization.is_some()`).
    ///
    /// #418 — the `hnsw` (f32) parameter is ACCEPTED for back-compat with the
    /// snapshot codec signature but is DROPPED immediately: a fitted quantized
    /// adapter never reads the f32 graph post-fit (every search/insert is
    /// gated through `quantized_active()`), so retaining it would defeat the
    /// whole purpose of SQ8 (#412 found SQ8 RSS > f32 RSS for exactly this
    /// reason — the f32 graph stayed resident on top of the u8 graph). We
    /// install `None` here, identical to the in-memory post-fit state.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts_with_quantization(
        dim: u32,
        metric: VectorMetric,
        ef_search: usize,
        hnsw: Arc<Hnsw<'static, f32, ShamirDist>>,
        rid_map: scc::HashMap<usize, RecordId, THasher>,
        rid_to_internal: scc::HashMap<RecordId, usize, THasher>,
        vectors: scc::HashMap<usize, Vec<f32>, THasher>,
        deleted: scc::HashMap<usize, (), THasher>,
        next_id: usize,
        hnsw_u8: Arc<Hnsw<'static, u8, ShamirDistU8>>,
        quantizer: Arc<Sq8Quantizer>,
        vectors_u8: scc::HashMap<usize, Vec<u8>, THasher>,
    ) -> Self {
        // Drop the f32 graph immediately — see the doc comment above.
        drop(hnsw);
        #[allow(clippy::disallowed_methods)] // O(N) ack: one-time seed at snapshot load
        let deleted_cnt = deleted.len();
        Self {
            dim,
            metric,
            ef_search,
            // Mark the adapter as quantization-enabled so `quantized_active`
            // (which gates on `is_fitted && quantization.is_some()`) returns
            // true once `is_fitted` is flipped below.
            quantization: Some(VectorQuantization::Sq8),
            // #418 — f32 graph freed on load: a fitted quantized adapter never
            // reads it. `ArcSwapOption::empty()` == `None`.
            hnsw: ArcSwapOption::empty(),
            rid_map,
            rid_to_internal,
            vectors,
            deleted,
            deleted_count: AtomicUsize::new(deleted_cnt),
            next_id: AtomicUsize::new(next_id),
            compaction_deleted_rids: None,
            // Install the u8 graph under the ArcSwapOption. We use `store`
            // (Release) so the happens-before edge pairs with the Acquire
            // load in `hnsw_u8_handle`.
            hnsw_u8: arc_swap::ArcSwapOption::from(Some(hnsw_u8)),
            vectors_u8,
            // Publish the quantizer via the OnceLock — it is a write-once
            // slot, and this is the single publish point on the load path.
            quantizer: {
                let lock = std::sync::OnceLock::new();
                let _ = lock.set(quantizer);
                lock
            },
            is_fitted: AtomicBool::new(true),
            fit_in_flight: AtomicBool::new(false),
            // Loaded already-fitted: every internal is in the graph.
            // The fit catch-up loop never runs on a loaded adapter
            // (`is_quantized()` is already true → the deferred-fit guard
            // is a no-op), so the value is inert here; set to `next_id`
            // for invariant clarity.
            migrated_pre_flip: AtomicUsize::new(next_id),
            next_id_at_flip: AtomicUsize::new(usize::MAX),
        }
    }

    /// Number of tombstoned internal ids (O(1) atomic mirror).
    #[allow(dead_code)] // API for #408 compaction trigger
    pub(crate) fn deleted_count(&self) -> usize {
        self.deleted_count.load(Ordering::Relaxed)
    }

    /// Number of live vectors (O(1) = next_id - deleted_count).
    ///
    /// `saturating_sub`: `next_id` and `deleted_count` are two independent
    /// Relaxed loads. `next_id` is bumped BEFORE its tombstone increment, so
    /// globally `deleted_count <= next_id` always holds — but a reader can
    /// observe a stale `next_id` with a fresher `deleted_count` under
    /// concurrent replace/delete, which would underflow a plain subtraction.
    /// Saturating keeps the heuristic cardinality at 0 in that transient
    /// window instead of panicking (debug) / wrapping to usize::MAX (release).
    #[allow(dead_code)] // API for #408 compaction trigger
    pub(crate) fn live_count(&self) -> usize {
        self.next_id
            .load(Ordering::Relaxed)
            .saturating_sub(self.deleted_count.load(Ordering::Relaxed))
    }

    /// Ratio of tombstoned slots to total allocated ids (0.0 when empty).
    pub(crate) fn deleted_ratio(&self) -> f64 {
        let next = self.next_id.load(Ordering::Relaxed);
        if next == 0 {
            return 0.0;
        }
        self.deleted_count.load(Ordering::Relaxed) as f64 / next as f64
    }

    /// V4.2 (#408) — Collect all live (non-tombstoned) (rid, vector) pairs.
    /// O(N) scan — called once per compaction, NOT on hot path.
    ///
    /// V5.2 (#411) — post-fit: returns DEQUANTIZED f32 vectors (the codes
    /// in `vectors_u8` are dequantized through the frozen quantizer). The
    /// compaction rebuild target is always an unquantized adapter (the
    /// compaction path does not preserve quantization in #411 — a
    /// quantization-aware compaction is #412), so returning f32 vectors
    /// is correct: the target adapter re-fits at its own threshold.
    pub(crate) fn collect_live_vectors(&self) -> Vec<(RecordId, Vec<f32>)> {
        let mut result: Vec<(RecordId, Vec<f32>)> = Vec::new();
        // Post-fit: dequantize codes. Pre-fit/unquantized: f32 buffer.
        let quantizer = if self.is_quantized() {
            self.quantizer.get().cloned()
        } else {
            None
        };
        self.rid_to_internal.scan(|rid, internal| {
            // Skip tombstoned internals
            if self.deleted.contains(internal) {
                return;
            }
            if let Some(q) = quantizer.as_ref() {
                if let Some(codes) = self.vectors_u8.read(internal, |_, c| c.clone()) {
                    result.push((*rid, q.dequantize(&codes)));
                }
            } else if let Some(vec) = self.vectors.read(internal, |_, v| v.clone()) {
                result.push((*rid, vec));
            }
        });
        result
    }

    /// V4.2 (#408) — Insert (rid, vec) ONLY if rid is absent from
    /// `rid_to_internal` AND absent from `compaction_deleted_rids`.
    /// Uses `entry_async` for atomic check-and-insert per rid.
    pub(crate) async fn backfill_if_absent(
        &self,
        items: &[(RecordId, Vec<f32>)],
    ) -> Result<(), VectorError> {
        for (rid, vec) in items {
            // Skip if this rid was deleted via double-write
            if let Some(ref del_rids) = self.compaction_deleted_rids {
                if del_rids.contains(rid) {
                    continue;
                }
            }
            // Atomic check: only insert if absent
            use scc::hash_map::Entry::{Occupied, Vacant};
            match self.rid_to_internal.entry_async(*rid).await {
                Occupied(_) => continue, // double-write already placed a fresher value
                Vacant(vac) => {
                    // Re-check deleted_rids under the entry lock to close the race
                    if let Some(ref del_rids) = self.compaction_deleted_rids {
                        if del_rids.contains(rid) {
                            continue;
                        }
                    }
                    let internal = self.next_id.fetch_add(1, Ordering::Relaxed);
                    vac.insert_entry(internal);

                    // Insert into graph. backfill runs on a compaction target
                    // (always a non-quant adapter), so the f32 graph is always
                    // present; None here is an invariant violation surfaced as
                    // an error, not a normal-path panic.
                    let hnsw = self.hnsw.load_full().ok_or_else(|| {
                        VectorError::Internal(
                            "backfill_if_absent: f32 graph absent on compaction target".into(),
                        )
                    })?;
                    let vec_owned = vec.clone();
                    tokio::task::spawn_blocking(move || {
                        hnsw.insert((&vec_owned, internal));
                    })
                    .await
                    .map_err(|e| VectorError::Internal(e.to_string()))?;

                    let _ = self.vectors.insert_async(internal, vec.clone()).await;
                    let _ = self.rid_map.insert_async(internal, *rid).await;
                }
            }
        }
        Ok(())
    }

    /// V4.2 (#408) — Clone the build config for compaction rebuild.
    pub(crate) fn build_config(&self) -> HnswConfig {
        HnswConfig {
            max_elements: self
                .next_id
                .load(Ordering::Relaxed)
                .saturating_sub(self.deleted_count.load(Ordering::Relaxed))
                .max(1000)
                + 1000, // 10%+ buffer
            m: 16,
            max_layer: 16,
            ef_construction: 200,
            ef_search: self.ef_search,
        }
    }

    /// V4.2 (#408) — Create a new empty adapter suitable as a compaction target,
    /// with `compaction_deleted_rids` set to Some.
    pub(crate) fn new_compaction_target(
        dim: u32,
        metric: VectorMetric,
        config: HnswConfig,
    ) -> Self {
        let mut adapter = Self::new(dim, metric, config);
        adapter.compaction_deleted_rids =
            Some(Arc::new(scc::HashMap::with_hasher(THasher::default())));
        adapter
    }

    // ===================================================================
    // V5.2 (#411) — SQ8 quantized u8-graph path.
    //
    // The adapter holds TWO graphs in Option slots: the legacy f32 graph
    // (`hnsw`) which is always present, and a u8 graph (`hnsw_u8`) which
    // is built once at the fit threshold. The `is_fitted` atomic is the
    // publish flag; once `true`, every insert and every search goes through
    // the u8 graph + rescore. See the struct doc on `HnswAdapter` for the
    // concurrency argument.
    // ===================================================================

    /// `true` when the adapter has crossed the fit threshold and the u8
    /// graph is live. `false` for unquantized adapters (always f32) and
    /// for quantized adapters below the threshold.
    pub(crate) fn is_quantized(&self) -> bool {
        self.is_fitted.load(Ordering::Acquire)
    }

    /// The opt-in quantization mode the adapter was constructed with.
    /// `None` = legacy f32 path, bit-for-bit.
    #[allow(dead_code)] // API for #412 snapshot codec
    pub(crate) fn quantization_mode(&self) -> Option<VectorQuantization> {
        self.quantization
    }

    /// Read-only handle to the frozen quantizer, if fitted. `None` for
    /// unquantized adapters and pre-fit quantized adapters. Used by the
    /// snapshot codec (#412) and introspection.
    #[allow(dead_code)] // API for #412 snapshot codec
    pub(crate) fn quantizer(&self) -> Option<&Arc<Sq8Quantizer>> {
        self.quantizer.get()
    }

    /// Read-only handle to the u8 graph, if fitted.
    #[allow(dead_code)] // API for #412 snapshot codec
    pub(crate) fn hnsw_u8_handle(&self) -> Option<Arc<Hnsw<'static, u8, ShamirDistU8>>> {
        self.hnsw_u8.load_full()
    }

    /// Iterate `(internal -> u8 codes)` pairs for snapshot serialisation
    /// (#412). No-op for unquantized / pre-fit adapters.
    #[allow(dead_code)] // API for #412 snapshot codec
    pub(crate) fn for_each_vector_u8<F: FnMut(usize, &[u8])>(&self, mut f: F) {
        if !self.is_quantized() {
            return;
        }
        self.vectors_u8.scan(|internal, codes| {
            f(*internal, codes);
        });
    }

    /// Quantize an f32 vector to u8 codes using the frozen quantizer.
    ///
    /// # Panics
    ///
    /// Panics if the adapter is not fitted (callers must gate on
    /// [`Self::is_quantized`]).
    fn quantize(&self, vec: &[f32]) -> Vec<u8> {
        let q = self.quantizer.get().expect("quantize called before fit");
        q.quantize(vec)
    }

    /// #423 (Б-1) — Canonical "publish codes + claim graph membership" op.
    ///
    /// Inserts `(internal, codes)` into `vectors_u8` via the atomic
    /// `scc::HashMap::insert` (lock-free CAS on the bucket slot) and
    /// returns `Some(())` IFF this call won the claim — i.e. `internal`
    /// was ABSENT from `vectors_u8` before this call. Returns `None` if
    /// `internal` was already present (another caller, or an earlier pass
    /// of the fit, already published codes for this internal).
    ///
    /// **Why the return value is the dedup authority for graph inserts.**
    /// A u8 graph node is identified by its `d_id == internal`. Inserting
    /// the same `d_id` twice creates a DUPLICATE node (a bug class we have
    /// hit before — `concurrent_upsert_across_threshold_no_duplicate_rids`).
    /// `hnsw_rs` exposes no `contains(id)` query, so we cannot ask the
    /// graph itself. Instead we make the `vectors_u8` slot the SINGLE
    /// source of truth: the caller that wins the `Ok` claim is the ONE
    /// caller entitled to insert the graph node. Every other caller
    /// observes `None` and skips the graph insert. Because
    /// `scc::HashMap::insert` is atomic per key, exactly one caller wins
    /// per `internal` — no double-insert is possible, regardless of
    /// concurrency between the fit catch-up loop, the fit snapshot/delta
    /// passes, and the upsert self-migration path.
    ///
    /// **Б-3 convergence:** a winning claim for an `internal <
    /// next_id_at_flip` (read from the adapter field, set by the fitter at
    /// the flip) also bumps `migrated_pre_flip`. This gives the catch-up
    /// loop an O(1) convergence signal that counts ONLY pre-flip internals
    /// and cannot be inflated by post-flip upserts (which allocate
    /// internals `>= next_id_at_flip`). Before the flip, `next_id_at_flip`
    /// is `usize::MAX` (sentinel), so pre-flip snapshot/delta claims (which
    /// happen before the field is set to the real value) would wrongly bump
    /// the counter — but the counter is only READ after the flip, and the
    /// fitter resets it to 0 at the flip, so the spurious pre-flip bumps are
    /// discarded (see `try_fit_and_rebuild`).
    ///
    /// `codes` is always consumed into `vectors_u8` on the winning path.
    /// On the losing path (`None`) the caller's `codes` are dropped — the
    /// already-present entry is authoritative (both are quantizations of
    /// the same vector under the same frozen quantizer, so they are
    /// bit-identical; keeping the existing one is correct).
    #[allow(clippy::disallowed_methods)] // scc::HashMap::insert is lock-free (CAS)
    fn claim_and_publish_u8(&self, internal: usize, codes: Vec<u8>) -> Option<()> {
        if self.vectors_u8.insert(internal, codes).is_ok() {
            // Won the claim — this internal is new to vectors_u8. Bump
            // the pre-flip migration counter if this is a pre-flip internal.
            let nif = self.next_id_at_flip.load(Ordering::Acquire);
            if internal < nif {
                self.migrated_pre_flip.fetch_add(1, Ordering::Relaxed);
            }
            Some(())
        } else {
            None
        }
    }

    /// Async twin of [`claim_and_publish_u8`] for the upsert paths (which
    /// run inside an async context and prefer `insert_async` to avoid the
    /// synchronous bucket spin on contention). Identical semantics: wins
    /// iff the slot was vacant, bumps `migrated_pre_flip` for pre-flip
    /// internals. See [`claim_and_publish_u8`] for the dedup authority
    /// argument.
    async fn claim_and_publish_u8_async(&self, internal: usize, codes: Vec<u8>) -> Option<()> {
        if self.vectors_u8.insert_async(internal, codes).await.is_ok() {
            let nif = self.next_id_at_flip.load(Ordering::Acquire);
            if internal < nif {
                self.migrated_pre_flip.fetch_add(1, Ordering::Relaxed);
            }
            Some(())
        } else {
            None
        }
    }

    /// #423 (Б-3) — bump `migrated_pre_flip` for a NEWLY-tombstoned
    /// pre-flip internal. Call ONLY after the `deleted.insert[_async]` that
    /// tombstoned `internal` returned `Ok` (i.e. this call won the
    /// tombstone) — mirrors [`claim_and_publish_u8`]'s claim-then-bump
    /// pattern so a tombstone counts toward fit convergence exactly once,
    /// exactly like a claim does. A tombstoned internal never ALSO wins a
    /// `vectors_u8` claim (every claim path filters `deleted.contains`
    /// first), so this and `claim_and_publish_u8[_async]` bump disjoint
    /// events for the same `internal` — no double-counting.
    ///
    /// Without this, a pre-flip internal tombstoned AFTER the fit-transition
    /// flip (a concurrent `delete`/rid-replacing `upsert` racing the fit)
    /// would never be claimed (the catch-up scan skips `deleted` entries)
    /// and the catch-up loop would spin forever waiting for a target that
    /// excluded it. See the seed comment in `try_fit_and_rebuild`.
    fn bump_migrated_on_tombstone(&self, internal: usize) {
        let nif = self.next_id_at_flip.load(Ordering::Acquire);
        if internal < nif {
            self.migrated_pre_flip.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `true` when the adapter should run the u8 graph path. Equivalent to
    /// `is_quantized()` but inlined for the hot search/insert branches.
    #[inline]
    fn quantized_active(&self) -> bool {
        // ACQUIRE (required): the fit publishes `hnsw_u8.store(Some)` (Release
        // via ArcSwapOption) BEFORE `is_fitted.store(true, Release)`. A reader
        // that observes `is_fitted == true` then does `hnsw_u8.load_full()`
        // and `.expect`s a live graph — so seeing `true` MUST happens-after the
        // graph store. Only an Acquire load of `is_fitted` establishes that
        // edge (Acquire-load synchronizes-with the Release-store, ordering all
        // of the fit thread's prior writes — incl. the graph store — before the
        // reader's subsequent `load_full`). A Relaxed load here is a data race:
        // it let a concurrent upsert observe `is_fitted == true` while
        // `hnsw_u8` still read `None`, panicking the `expect`
        // (`concurrent_upsert_across_threshold_no_loss` under nextest load).
        // A false negative (stale `false`) only routes ONE request through the
        // still-live f32 path — harmless. false→true flips exactly once.
        self.is_fitted.load(Ordering::Acquire) && self.quantization.is_some()
    }

    /// Try to fit the SQ8 quantizer and build the u8 graph.
    ///
    /// Triggered by `upsert`/`upsert_batch` when the live-element count
    /// crosses [`FIT_THRESHOLD`] AND `quantization == Some(Sq8)`.
    ///
    /// # Concurrency (class #408 — no lost mutation)
    ///
    /// A single-flight guard ([`Self::fit_in_flight`]) ensures exactly
    /// ONE caller performs the rebuild. The rebuild proceeds in three
    /// phases, ALL inside ONE `spawn_blocking` so no `.await` holds a
    /// guard:
    ///
    ///  1. **Snapshot** the f32 buffer under a brief scc read-cursor,
    ///     recording the set of live internals seen.
    ///  2. **Fit + build**: fit `Sq8Quantizer` on the snapshot, quantize
    ///     each f32 vector, and `parallel_insert` the codes into a fresh
    ///     `Hnsw<'static, u8, ShamirDistU8>`.
    ///  3. **Delta re-insert**: any upsert that arrived BETWEEN the
    ///     snapshot and the mutex-publish of the new graph added its f32
    ///     vector to the buffer (the upsert path always appends to the
    ///     buffer before checking `is_fitted`). We scan the buffer again
    ///     and quantize+insert any internal NOT already in the u8 graph.
    ///     This closes the race: no f32 vector added during the rebuild
    ///     is lost.
    ///
    /// The publish (step 3's completion) sets `is_fitted = true` with
    /// `Release` ordering, which pairs with the `Acquire` in
    /// [`Self::is_quantized`]. After publish, every subsequent upsert
    /// goes through the post-fit path (quantize + insert into the u8
    /// graph directly).
    ///
    /// **#423 (Б-1 + Б-3) — graph connectivity + exact convergence.**
    /// A catch-up loop runs post-publish to drain pre-flip in-flight
    /// upserts. Every code that enters `vectors_u8` — whether via the
    /// snapshot pass, the delta pass, the catch-up loop, or the upsert
    /// self-migration path — claims its `vectors_u8` slot through
    /// [`Self::claim_and_publish_u8`], which is the SINGLE authority for
    /// "this internal's graph node has been inserted" (see its doc). The
    /// catch-up loop accumulates the codes that won the claim but had no
    /// graph node yet (the snapshot/delta passes inserted graph nodes
    /// for their own claims; the catch-up and self-migration claims are
    /// the ones that still need a node) and performs ONE final
    /// `parallel_insert` into `hnsw_u8` before dropping the f32 graph.
    /// Convergence is counted by `migrated_pre_flip` (only internals
    /// `< next_id_at_flip`), NOT by `vectors_u8.len()` — see Б-3.
    ///
    /// Returns `Ok(())` if the fit completed OR if another caller was
    /// already fitting (the single-flight loser). Errors only on graph
    /// build failure (a `spawn_blocking` panic).
    async fn try_fit_and_rebuild(&self) -> Result<(), VectorError> {
        // Only quantization-enabled adapters ever fit.
        if self.quantization.is_none() {
            return Ok(());
        }
        // Single-flight: the first caller to CAS false→true wins.
        if self
            .fit_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return Ok(()); // another caller is fitting
        }

        // Drop-guard: no matter how we exit, clear the flag so a later
        // threshold crossing (e.g. after a mass delete + re-insert) can
        // fit again. (In #411 fit is one-shot — once fitted, the flag
        // stays true — but the guard is defensive.)
        struct FitGuard<'a>(&'a AtomicBool);
        impl Drop for FitGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = FitGuard(&self.fit_in_flight);

        // Snapshot the f32 buffer (internal, vec) pairs. We read under a
        // plain scc scan — no entry guard held across the await below.
        let mut snapshot: Vec<(usize, Vec<f32>)> = Vec::new();
        self.vectors.scan(|internal, vec| {
            // Skip tombstoned internals — their codes must NOT enter the
            // u8 graph (a tombstoned internal is never resurrectated:
            // next_id is monotonic, so a brand-new internal never aliases
            // a tombstoned one).
            if !self.deleted.contains(internal) {
                snapshot.push((*internal, vec.clone()));
            }
        });
        // The snapshot is empty or below threshold → nothing to fit yet
        // (the threshold check is the caller's responsibility, but we
        // defend in depth).
        if snapshot.len() < FIT_THRESHOLD {
            return Ok(());
        }

        let dim = self.dim as usize;
        let metric = self.metric;
        let ef_construction = self.ef_search.max(64);
        let m = 16usize;
        let max_layer = 16usize;

        // Phase 1+2: fit + build. The graph INSERT is deferred until AFTER
        // the snapshot codes are claimed into `vectors_u8` so that the
        // claim (Ok/Err) is the exact authority for "does this internal
        // already have a graph node" — see [`claim_and_publish_u8`].
        let training: Vec<Vec<f32>> = snapshot.iter().map(|(_, v)| v.clone()).collect();
        // #418 — the f32 graph is NO LONGER retained here. The pre-#418 code
        // held an explicit `Arc::clone(&self.hnsw)` "to keep the f32 graph
        // alive", which was exactly the bug that defeated SQ8's memory win
        // (the graph stayed resident for the adapter's lifetime). We now drop
        // it post-fit (see the drop at the end of this fn) — the snapshot
        // already captured everything the u8 graph needs.

        let quantizer_arc = Arc::new(Sq8Quantizer::fit(&training, dim));
        let dist = ShamirDistU8::new(Arc::clone(&quantizer_arc), metric);

        // Quantize the snapshot and CLAIM each internal into vectors_u8.
        // Pre-flip: `next_id_at_flip` is still `usize::MAX` (sentinel), so
        // the claims DO bump `migrated_pre_flip` spuriously — but the
        // fitter OVERWRITES the counter with the correct seed at the flip
        // (see the flip block below), discarding those bumps. The snapshot
        // pass is the FIRST claimant for these internals (they were in
        // `vectors` at snapshot time and no post-flip path has run —
        // `is_fitted` is still false), so every claim here wins Ok and the
        // codes enter vectors_u8. `snapshot_claimed` tallies the winners
        // for the Б-3 convergence seed.
        let mut graph_batch_snapshot: Vec<(usize, Vec<u8>)> = Vec::with_capacity(snapshot.len());
        let mut snapshot_claimed = 0usize;
        for (internal, vec) in &snapshot {
            let codes = quantizer_arc.quantize(vec);
            if self
                .claim_and_publish_u8(*internal, codes.clone())
                .is_some()
            {
                graph_batch_snapshot.push((*internal, codes));
                snapshot_claimed += 1;
            }
        }

        // Phase 3: delta re-insert. Collect any internals that arrived
        // during the build (they're in `vectors` but not in our snapshot)
        // and claim them into vectors_u8 + the graph batch.
        let snapshot_ids: shamir_collections::TFxSet<usize> =
            snapshot.iter().map(|(i, _)| *i).collect();
        let mut graph_batch_delta: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut delta_claimed = 0usize;
        self.vectors.scan(|internal, vec| {
            if !snapshot_ids.contains(internal) && !self.deleted.contains(internal) {
                let codes = quantizer_arc.quantize(vec);
                if self
                    .claim_and_publish_u8(*internal, codes.clone())
                    .is_some()
                {
                    graph_batch_delta.push((*internal, codes));
                    // SAFETY: this closure is `FnMut`-safe for a plain
                    // `usize` counter increment (no await, no guard).
                    delta_claimed += 1;
                }
            }
        });

        // Build the u8 graph and insert the snapshot + delta codes in ONE
        // `parallel_insert` (the delta codes arrived during the build; both
        // sets were claimed above so there is exactly one node per internal).
        let mut graph_batch_for_build: Vec<(usize, Vec<u8>)> = graph_batch_snapshot;
        graph_batch_for_build.append(&mut graph_batch_delta);
        let dist_for_build = dist.clone();
        let (hnsw_u8, build_result): (
            Arc<Hnsw<'static, u8, ShamirDistU8>>,
            Result<(), VectorError>,
        ) = {
            let result = tokio::task::spawn_blocking(move || {
                let hnsw = Hnsw::<u8, ShamirDistU8>::new(
                    m,
                    graph_batch_for_build.len().max(1000) + 1000,
                    max_layer,
                    ef_construction,
                    dist_for_build,
                );
                let batch: Vec<(&Vec<u8>, usize)> = graph_batch_for_build
                    .iter()
                    .map(|(internal, codes)| (codes, *internal))
                    .collect();
                hnsw.parallel_insert(&batch);
                Arc::new(hnsw)
            })
            .await;
            match result {
                Ok(h) => (h, Ok(())),
                Err(e) => {
                    return Err(VectorError::Internal(format!(
                        "fit spawn_blocking join error: {e}"
                    )))
                }
            }
        };
        build_result?;

        // === PUBLISH quantizer (needed for self-migration quantize) ===
        let _ = self.quantizer.set(Arc::clone(&quantizer_arc));

        // === FLIP is_fitted + store graph ===
        // After this, new upserts go u8-path. In-flight pre-flip upserts
        // continue inserting into `vectors` (caught by catch-up below).
        //
        // Capture next_id at flip time: all internals < this value were
        // allocated pre-flip and must end up in vectors_u8 (unless
        // tombstoned). Used by the catch-up convergence check (Б-3).
        let next_id_at_flip = self.next_id.load(Ordering::Acquire);
        // Deleted count AT FLIP: every tombstone counted here is of an
        // internal `< next_id_at_flip` (an internal must exist — i.e. have
        // been allocated by `next_id.fetch_add`, which only increases —
        // before it can be tombstoned, and `next_id` is monotonic, so any
        // tombstone visible at this Acquire load happened against an
        // internal that already existed when `next_id_at_flip` was read).
        // Folded into the `migrated_pre_flip` seed below — see the seed
        // comment for why pre-flip tombstones are counted as "accounted
        // for" alongside pre-flip claims.
        let deleted_count_at_flip = self.deleted_count.load(Ordering::Acquire);
        // Б-3 convergence seed + target. Every pre-flip internal (id <
        // next_id_at_flip) ends up in EXACTLY ONE of two disjoint states:
        // (a) claimed into vectors_u8 (via snapshot/delta/catch-up/
        //     self-migration — all funnel through `claim_and_publish_u8`,
        //     which bumps `migrated_pre_flip` for `internal <
        //     next_id_at_flip`), or
        // (b) tombstoned (via `delete`/rid-replacing `upsert`/
        //     `upsert_batch` — these ALSO bump `migrated_pre_flip` for
        //     `old_internal < next_id_at_flip`, using the exact same
        //     sentinel-then-seed trick as claims: see those call sites).
        // A tombstoned internal is excluded from the live snapshot/delta
        // scan (`self.deleted.contains` guards both), so (a) and (b) never
        // double-count the SAME internal — the seed below is exact, not
        // an over/under-approximation.
        //
        // The ORIGINAL Б-3 fix (a single reviewer round before this one)
        // subtracted a FROZEN `deleted_count_at_flip` from `target` instead
        // of counting tombstones into `migrated_pre_flip` — that undercounts
        // `target` correctly for tombstones that happened BEFORE the flip,
        // but any tombstone of a pre-flip internal AFTER the flip (a
        // concurrent `delete`/rid-collision racing the fit) would never be
        // claimed (the catch-up scan skips `deleted` entries) and never
        // bump the frozen target — the loop would spin forever (CONFIRMED
        // adversarial-review finding). Folding tombstones into the SAME
        // live counter as claims closes that gap: a post-flip tombstone of
        // a pre-flip internal now bumps `migrated_pre_flip` directly, so
        // convergence is reachable regardless of WHEN the tombstone lands.
        //
        // Seed = (claims already won at flip time) + (tombstones already
        // recorded at flip time); target = next_id_at_flip (no
        // subtraction — tombstones now count INTO the accumulator instead
        // of OUT of the target). The seed is written as a plain `store`
        // (not `fetch_add`): no concurrent self-migration or tombstone
        // bump can be lost by this overwrite, because both gate on
        // `next_id_at_flip` via an `Acquire` load, and that load cannot
        // observe the real (non-sentinel) value until AFTER this store
        // has completed — `next_id_at_flip.store` (below, `Release`) is
        // strictly ORDERED AFTER this store in program order on this
        // thread, and the standard release-sequence guarantee means any
        // thread whose `Acquire` load of `next_id_at_flip` observes the
        // real value also observes every write (including this one) that
        // preceded the `next_id_at_flip` release-store in program order.
        // So a concurrent claim/tombstone can only ever see (and
        // `fetch_add` on top of) the seed — never race under it.
        self.migrated_pre_flip.store(
            snapshot_claimed + delta_claimed + deleted_count_at_flip,
            Ordering::Release,
        );
        self.next_id_at_flip
            .store(next_id_at_flip, Ordering::Release);
        self.hnsw_u8.store(Some(Arc::clone(&hnsw_u8)));
        self.is_fitted.store(true, Ordering::Release);

        // === CONVERGING CATCH-UP LOOP (post-publish) ===
        //
        // In-flight pre-flip upserts finish inserting into `vectors`. The
        // upsert self-migration path AND this loop both claim their codes
        // into vectors_u8 via [`claim_and_publish_u8`]; whichever wins the
        // claim owns the graph-node insert. This loop accumulates the codes
        // whose claim it WON (and which therefore still need a graph node)
        // into `pending_graph_inserts`; a single `parallel_insert` after
        // the loop installs all of them at once. Self-migration winners
        // insert their own single node via `quantize_and_insert_u8`.
        //
        // Convergence (Б-3): count ONLY pre-flip internals that have landed
        // in vectors_u8 OR been tombstoned (`migrated_pre_flip`), NOT
        // `vectors_u8.len()`. The latter is inflated by post-flip upserts
        // (internals >= next_id_at_flip land directly in vectors_u8),
        // which would make the loop exit early — before a pre-flip
        // upsert's `vectors.insert` has run, dropping the f32 graph under
        // its feet (Б-3 → "f32 graph absent" Internal error).
        // `migrated_pre_flip` is bumped exactly once per distinct pre-flip
        // internal — once via a winning `claim_and_publish_u8[_async]`
        // call, or once via a tombstone site (`delete`/rid-replacing
        // `upsert`/`upsert_batch`) — so it reaches `next_id_at_flip`
        // precisely when every pre-flip internal has been claimed or
        // tombstoned (see the seed comment above for why these two sets
        // are disjoint and jointly exhaustive).
        let mut pending_graph_inserts: Vec<(usize, Vec<u8>)> = Vec::new();
        // Target: every pre-flip internal — no subtraction. Tombstones now
        // count INTO `migrated_pre_flip` (see above), not out of the
        // target, so the target is simply the count of internals allocated
        // before the flip.
        let target = next_id_at_flip;
        loop {
            // Claim any pre-flip internals still sitting in the f32 buffer.
            self.vectors.scan(|internal, vec| {
                if !self.deleted.contains(internal) {
                    let codes = quantizer_arc.quantize(vec);
                    if self
                        .claim_and_publish_u8(*internal, codes.clone())
                        .is_some()
                    {
                        // We won the claim → we owe a graph node. Stash
                        // for the final batch insert after the loop.
                        pending_graph_inserts.push((*internal, codes));
                    }
                }
            });

            // Drain entries that are in vectors_u8 (claimed by us or by a
            // concurrent self-migration upsert). Both paths remove from
            // `vectors`; whichever runs first wins, the second is a no-op.
            let mut internals_to_drop: Vec<usize> = Vec::new();
            self.vectors.scan(|internal, _| {
                if self.vectors_u8.contains(internal) {
                    internals_to_drop.push(*internal);
                }
            });
            for internal in &internals_to_drop {
                let _ = self.vectors.remove(internal);
            }

            // Convergence: migrated_pre_flip is bumped only for claims on
            // internals < next_id_at_flip (see claim_and_publish_u8). When
            // it reaches `target`, every non-tombstoned pre-flip internal
            // has been claimed into vectors_u8 — by this loop OR by a
            // concurrent self-migration upsert (both bump the same
            // counter, since both go through claim_and_publish_u8).
            let migrated = self.migrated_pre_flip.load(Ordering::Acquire);
            if migrated >= target {
                break;
            }

            tokio::task::yield_now().await;
        }

        // === #423 (Б-1) — FINAL GRAPH CONNECTIVITY INSERT ===
        //
        // Every internal that entered `vectors_u8` via THIS fit (snapshot,
        // delta, and catch-up passes above) won its claim and is owed a
        // graph node. The snapshot + delta nodes were inserted in the build
        // `parallel_insert`; the catch-up claims are collected in
        // `pending_graph_inserts`. Internals claimed by a concurrent
        // self-migration upsert inserted their OWN single node (and are NOT
        // in `pending_graph_inserts` — we lost those claims). So this batch
        // contains exactly the nodes we owe, no duplicates, no omissions.
        //
        // This runs BEFORE `self.hnsw.store(None)`: by the time the f32
        // graph is dropped, every internal in `vectors_u8` has a node in
        // `hnsw_u8`. Before this fix, the catch-up loop and self-migration
        // populated `vectors_u8` WITHOUT inserting graph nodes — a vector
        // was invisible to graph-search and co-filter forever, and rode
        // into the v2 snapshot (which dumps hnsw_u8) as a hole.
        if !pending_graph_inserts.is_empty() {
            let hnsw_u8_for_catchup = Arc::clone(&hnsw_u8);
            tokio::task::spawn_blocking(move || {
                let batch: Vec<(&Vec<u8>, usize)> = pending_graph_inserts
                    .iter()
                    .map(|(internal, codes)| (codes, *internal))
                    .collect();
                hnsw_u8_for_catchup.parallel_insert(&batch);
            })
            .await
            .map_err(|e| VectorError::Internal(format!("catch-up graph insert join error: {e}")))?;
        }

        // === #418 — DROP THE F32 GRAPH ===
        //
        // At this point the post-fit path is FULLY published and converged:
        //  - `hnsw_u8` is live under the ArcSwapOption (Release store above).
        //  - `is_fitted == true` (Release store above) → every NEW search /
        //    upsert / delete is gated through `quantized_active()` and reads
        //    the u8 graph. No NEW caller can reach the f32 graph.
        //  - The catch-up loop above drained `vectors`: every pre-flip
        //    in-flight upsert has either landed in `vectors_u8` or been
        //    tombstoned. The f32 buffer is empty.
        //  - `vectors_u8` holds every live code AND every one of those
        //    codes has a node in `hnsw_u8` (the final connectivity insert
        //    above closed Б-1). `collect_live_vectors` dequantizes from
        //    `vectors_u8` (NOT from the f32 graph); the v2 snapshot dumps
        //    `hnsw_u8` + `vectors_u8`. The f32 graph is unobservable to
        //    every post-fit code path.
        //
        // `store(None)` is a Release store. Any in-flight pre-fit SEARCH that
        // already did `hnsw.load_full()` holds its own `Arc` clone and
        // finishes its traversal against the now-private graph; the `Arc`'s
        // refcount reaches 0 only after the last such reader drops its clone
        // — RCU, no UAF. (A pre-fit search that has NOT yet loaded sees
        // `is_fitted == true` on its next `quantized_active()` check and
        // routes through the u8 graph instead — the gate makes the f32 graph
        // unobservable to readers that have not already snapshotted it.)
        //
        // Unquantized adapters NEVER reach here (`try_fit_and_rebuild` returns
        // early when `quantization.is_none()`), so the f32 graph of a
        // non-quant adapter is retained for its full lifetime — bit-for-bit
        // back-compat.
        self.hnsw.store(None);

        Ok(())
    }

    /// Insert a single node `(codes, internal)` into the live u8 graph.
    /// CPU-bound (rayon `insert`) → `spawn_blocking`; the codes are tiny
    /// (dim bytes) so the clone is cheap. Used by [`quantize_and_insert_u8`]
    /// (post-fit single upsert) and by the self-migration path (which owns
    /// the node insert when it wins the `vectors_u8` claim — see Б-1).
    async fn insert_u8_graph_node(
        &self,
        internal: usize,
        codes: Vec<u8>,
    ) -> Result<(), VectorError> {
        let hnsw_u8 = self
            .hnsw_u8_handle()
            .ok_or_else(|| VectorError::Internal("u8 graph not fitted".into()))?;
        let codes_for_insert = codes.clone();
        tokio::task::spawn_blocking(move || {
            hnsw_u8.insert((&codes_for_insert, internal));
        })
        .await
        .map_err(|e| VectorError::Internal(format!("u8 insert join error: {e}")))?;
        Ok(())
    }

    /// Internal: post-fit insert path. Quantizes the vector and inserts
    /// the codes into the u8 graph + vectors_u8. Does NOT touch the f32
    /// buffer (post-fit the f32 buffer is empty and stays empty).
    ///
    /// Returns the codes (for the caller to publish into vectors_u8) on
    /// success. The caller owns the vectors_u8 insert (so it can batch).
    async fn quantize_and_insert_u8(
        &self,
        internal: usize,
        vec: &[f32],
    ) -> Result<Vec<u8>, VectorError> {
        let codes = self.quantize(vec);
        self.insert_u8_graph_node(internal, codes.clone()).await?;
        Ok(codes)
    }

    // ------------------------------------------------------------------
    // #424 (Б-4) — post-fit u8 search cores.
    //
    // These are the u8-graph / u8-brute-force bodies that the main
    // `quantized_active()` branches in `search` / `search_cofilter` inline.
    // Extracted as private helpers so the transient-None retry path (a
    // request that read `quantized_active() == false`, then raced a fit
    // flip that dropped the f32 graph before `hnsw.load_full()`) can re-run
    // the EXACT same u8 logic instead of returning an empty result.
    // ------------------------------------------------------------------

    /// Post-fit quantized search on a SMALL index (exact brute-force over
    /// dequantized u8 codes). Mirrors the `quantized_active() && len <=
    /// QUANT_BRUTE_FORCE_MAX` branch of [`Self::search`].
    ///
    /// `k` is the (already clamped) top-k; returns at most `k` pairs sorted
    /// ascending by distance. Tombstoned internals are skipped.
    async fn search_quantized_bruteforce(
        &self,
        query: &[f32],
        k: u32,
    ) -> Result<Vec<(RecordId, f32)>, VectorError> {
        let quantizer = self
            .quantizer
            .get()
            .expect("quantized_active but quantizer unset");
        let mut pairs: Vec<(usize, Vec<u8>)> = Vec::with_capacity(256);
        self.vectors_u8.scan(|i, c| pairs.push((*i, c.clone())));
        let mut out: Vec<(RecordId, f32)> = Vec::with_capacity(pairs.len());
        for (internal, codes) in pairs {
            if self.deleted.contains_async(&internal).await {
                continue;
            }
            if let Some(rid) = self.rid_map.read_async(&internal, |_, r| *r).await {
                let d = rescore_f32(self.metric, quantizer, query, &codes);
                out.push((rid, d));
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(k as usize);
        Ok(out)
    }

    /// Post-fit quantized search on a LARGE index (u8 HNSW graph traversal
    /// + exact f32 rescore). Mirrors the `quantized_active() && len >
    /// QUANT_BRUTE_FORCE_MAX` branch of [`Self::search`].
    ///
    /// `ef` is the (already clamped) ef_search floor; the actual graph
    /// exploration uses `max(ef, overscan)` where `overscan = 16k+64`.
    async fn search_quantized_graph(
        &self,
        query: &[f32],
        k: u32,
        ef: usize,
    ) -> Result<Vec<(RecordId, f32)>, VectorError> {
        let quantizer = self
            .quantizer
            .get()
            .expect("quantized_active but quantizer unset");
        let hnsw_u8 = self
            .hnsw_u8_handle()
            .expect("quantized_active but u8 graph unset");
        let query_codes = quantizer.quantize(query);
        let overscan = ((k as usize) * 16 + 64).min(MAX_TOPK as usize);
        let ef_q = ef.max(overscan);
        let neighbors =
            tokio::task::spawn_blocking(move || hnsw_u8.search(&query_codes, overscan, ef_q))
                .await
                .map_err(|e| VectorError::Internal(e.to_string()))?;

        let mut out: Vec<(RecordId, f32)> = Vec::with_capacity(k as usize + 16);
        for n in neighbors {
            if self.deleted.contains_async(&n.d_id).await {
                continue;
            }
            let codes_opt = self.vectors_u8.read_async(&n.d_id, |_, c| c.clone()).await;
            if let Some(codes) = codes_opt {
                if let Some(rid) = self.rid_map.read_async(&n.d_id, |_, v| *v).await {
                    let exact = rescore_f32(self.metric, quantizer, query, &codes);
                    out.push((rid, exact));
                }
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(k as usize);
        Ok(out)
    }

    /// Post-fit quantized co-filter search (u8 HNSW `search_filter` with an
    /// allow-set + exact f32 rescore). Mirrors the `quantized_active()`
    /// branch of [`Self::search_cofilter`].
    ///
    /// `ef` is the (already overscan-clamped) ef_search; `allow_set` is the
    /// internal-ID allow-set built from the candidate RIDs.
    async fn search_cofilter_quantized(
        &self,
        query: &[f32],
        k: u32,
        ef: usize,
        allow_set: &shamir_collections::TFxSet<usize>,
    ) -> Result<Vec<(RecordId, f32)>, VectorError> {
        let quantizer = self
            .quantizer
            .get()
            .expect("quantized_active but quantizer unset");
        let hnsw_u8 = self
            .hnsw_u8_handle()
            .expect("quantized_active but u8 graph unset");
        let query_codes = quantizer.quantize(query);
        let knbn = k as usize;
        // Clone the allow-set into an Arc so the spawn_blocking closure is
        // 'static. The set is typically small (the candidate RID count) so
        // the clone is cheap relative to the graph traversal.
        let allow = Arc::new(allow_set.clone());
        let neighbors = tokio::task::spawn_blocking(move || {
            let pred = |id: &usize| allow.contains(id);
            hnsw_u8.search_filter(&query_codes, knbn, ef, Some(&pred))
        })
        .await
        .map_err(|e| VectorError::Internal(e.to_string()))?;

        let mut out: Vec<(RecordId, f32)> = Vec::with_capacity(k as usize);
        for n in neighbors {
            let codes_opt = self.vectors_u8.read_async(&n.d_id, |_, c| c.clone()).await;
            if let Some(codes) = codes_opt {
                if let Some(rid) = self.rid_map.read_async(&n.d_id, |_, v| *v).await {
                    let exact = rescore_f32(self.metric, quantizer, query, &codes);
                    out.push((rid, exact));
                }
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(k as usize);
        Ok(out)
    }
}

/// Co-filter ef-overscan multiplier. `search_filter` can return < knbn under
/// tight filters (layer-0-only application + post-hoc drop). We compensate by
/// requesting `ef = max(ef_base, k * CO_FILTER_EF_MULTIPLIER)` so the graph
/// traversal explores enough candidates to fill the result.
///
/// 8× empirically covers 90%+ of cases where the allow-set is 1–5% of the
/// dataset (validated in the overscan contract test). Combined with the retry
/// widening in the engine, this ensures recall is not catastrophically degraded.
pub const CO_FILTER_EF_MULTIPLIER: u32 = 8;

/// Threshold: if the candidate set (from secondary index) has at most this many
/// RIDs, the pre-filter path (exact SIMD brute-force over the candidates) is
/// chosen. Above this threshold, co-filter (HNSW search_filter) is preferred.
///
/// 4096 is the sweet spot: exact SIMD over 4096 128-d vectors takes ~0.5 ms
/// (AVX2), which is competitive with a single HNSW search_filter call. Above
/// 4096 the linear scan becomes the bottleneck; below it, the graph overhead
/// (per-hop distance computations, cache misses) outweighs the brute-force.
pub const PRE_FILTER_MAX_CANDIDATES: usize = 4096;

/// Upper selectivity bound for co-filter. If the allow-set exceeds this
/// fraction of the total dataset, post-filter (V3.1 oversample-retry) is
/// preferred — co-filter gains diminish when most points pass the filter
/// anyway, and the ef-overscan cost is wasted.
pub const CO_FILTER_MAX_SELECTIVITY: f64 = 0.20;

impl HnswAdapter {
    /// **Pre-filter path** (V3.2): exact SIMD top-k scoring over a small
    /// candidate set of RIDs known to pass the residual predicate.
    ///
    /// The caller provides RIDs that passed the secondary index lookup. This
    /// method resolves them to internal IDs, retrieves their vectors, scores
    /// each against `query` using the SIMD kernels, and returns the top-k by
    /// distance (ascending). Result is EXACT (no approximation).
    ///
    /// Returns `Ok(vec![])` if none of the candidate RIDs have vectors.
    pub async fn search_prefilter(
        &self,
        query: &[f32],
        k: u32,
        candidates: &[RecordId],
    ) -> Result<Vec<(RecordId, f32)>, VectorError> {
        if query.len() as u32 != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len() as u32,
            });
        }
        let k = if k == 0 {
            return Ok(vec![]);
        } else {
            k.min(MAX_TOPK)
        };

        let dist = ShamirDist {
            metric: self.metric,
        };

        // V5.2 (#411) — post-fit: candidate vectors are u8 codes; dequant +
        // exact f32 distance for scoring. Pre-fit/unquantized: f32 vectors
        // straight from the buffer.
        let quantized = self.quantized_active();
        let quantizer = if quantized {
            Some(
                self.quantizer
                    .get()
                    .expect("quantized_active but quantizer unset"),
            )
        } else {
            None
        };

        let mut scored: Vec<(RecordId, f32)> = Vec::with_capacity(candidates.len());
        for &rid in candidates {
            let internal = match self.rid_to_internal.read_async(&rid, |_, v| *v).await {
                Some(i) => i,
                None => continue,
            };
            if self.deleted.contains_async(&internal).await {
                continue;
            }
            if let Some(q) = quantizer {
                // Post-fit: read codes, dequant + exact rescore.
                let codes_opt = self
                    .vectors_u8
                    .read_async(&internal, |_, c| c.clone())
                    .await;
                if let Some(codes) = codes_opt {
                    let d = rescore_f32(self.metric, q, query, &codes);
                    scored.push((rid, d));
                }
            } else {
                let vec_opt = self.vectors.read_async(&internal, |_, v| v.clone()).await;
                if let Some(v) = vec_opt {
                    let d = dist.eval(query, &v);
                    scored.push((rid, d));
                }
            }
        }

        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k as usize);
        Ok(scored)
    }

    /// **Co-filter path** (V3.2): HNSW `search_filter` with an allow-set of
    /// internal IDs derived from the candidate RIDs.
    ///
    /// Uses a closure-based `FilterT` (no sorting required). Applies generous
    /// ef-overscan ([`CO_FILTER_EF_MULTIPLIER`]) to compensate for the
    /// `search_filter` under-return behaviour documented in the V0.0 spike.
    pub async fn search_cofilter(
        &self,
        query: &[f32],
        k: u32,
        ef_search_override: Option<u32>,
        candidates: &[RecordId],
    ) -> Result<Vec<(RecordId, f32)>, VectorError> {
        if query.len() as u32 != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len() as u32,
            });
        }
        let k = if k == 0 {
            return Ok(vec![]);
        } else {
            k.min(MAX_TOPK)
        };

        // Build allow-set of internal IDs from the candidate RIDs.
        let mut allow_set = shamir_collections::TFxSet::<usize>::with_hasher(THasher::default());
        for &rid in candidates {
            if let Some(internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
                if !self.deleted.contains_async(&internal).await {
                    allow_set.insert(internal);
                }
            }
        }

        if allow_set.is_empty() {
            return Ok(vec![]);
        }

        // ef-overscan: generous to compensate search_filter under-return.
        let ef_base = match ef_search_override {
            Some(v) => (v.min(MAX_EF_SEARCH) as usize).max(k as usize),
            None => self.ef_search,
        };
        let ef = ef_base.max((k as usize) * CO_FILTER_EF_MULTIPLIER as usize);

        // V5.2 (#411) — post-fit: co-filter on the u8 graph, then rescore.
        // Pre-fit/unquantized: co-filter on the f32 graph as before.
        if self.quantized_active() {
            // #424 (Б-4) — body extracted to [`search_cofilter_quantized`]
            // so the transient-None retry path in the f32-graph branch below
            // can re-run the EXACT same logic instead of returning empty.
            return self
                .search_cofilter_quantized(query, k, ef, &allow_set)
                .await;
        }

        // #418 — f32 co-filter path. Reached ONLY when `!quantized_active()`
        // (the quantized branch returned above). In that state the f32 graph
        // is always present (unquantized adapters never drop it; quantized
        // adapters drop it only post-fit, which IS `quantized_active()`)...
        // UNLESS a concurrent fit transition raced between this function's
        // `quantized_active()` gate (above) and the `hnsw.load_full()` below.
        //
        // #424 (Б-4) — transient-None race window. Same window as `search`:
        //   1. THIS request read `quantized_active() == false` (pre-flip) in
        //      the `if` above, so it landed here on the f32 path.
        //   2. A concurrent fitter flipped `is_fitted = true` + dropped the
        //      f32 graph (`hnsw.store(None)`).
        //   3. THIS request's `hnsw.load_full()` now returns `None`.
        //
        // The old code returned `Ok(vec![])` — silently empty. Fix: re-check
        // `quantized_active()` and route through the u8 co-filter path if the
        // flip landed in the window. If still `false`, it is a genuine
        // invariant violation (unreachable) — defensive empty.
        //
        // #424 (Б-4) — test hook: same as `search` above. Pause between
        // the `quantized_active() == false` gate and `load_full()` so a
        // deterministic test can trigger the fit-drop in the window.
        #[cfg(test)]
        if let Some(gate) = TEST_SEARCH_F32_GATE.load_full() {
            gate.arrived
                .store(true, std::sync::atomic::Ordering::SeqCst);
            gate.notify.notified().await;
        }
        let hnsw = match self.hnsw.load_full() {
            Some(h) => h,
            None => {
                // #424 (Б-4) — transient flip during this request.
                if self.quantized_active() {
                    return self
                        .search_cofilter_quantized(query, k, ef, &allow_set)
                        .await;
                }
                // Genuinely unreachable: non-quantized / pre-flip adapter
                // with no f32 graph. Defensive empty on a read path.
                return Ok(vec![]);
            }
        };
        let query_owned = query.to_vec();
        let knbn = k as usize;

        let neighbors = tokio::task::spawn_blocking(move || {
            let pred = |id: &usize| allow_set.contains(id);
            hnsw.search_filter(&query_owned, knbn, ef, Some(&pred))
        })
        .await
        .map_err(|e| VectorError::Internal(e.to_string()))?;

        let mut out: Vec<(RecordId, f32)> = Vec::with_capacity(k as usize);
        for n in neighbors {
            if let Some(rid) = self.rid_map.read_async(&n.d_id, |_, v| *v).await {
                out.push((rid, n.distance));
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(k as usize);
        Ok(out)
    }
}

#[async_trait]
impl VectorAdapter for HnswAdapter {
    async fn upsert(&self, rid: RecordId, vec: &[f32]) -> Result<(), VectorError> {
        if vec.len() as u32 != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: vec.len() as u32,
            });
        }

        // D12: claim the rid slot atomically. Two concurrent upserts for the
        // SAME rid (reachable since III.5 moved HNSW promote outside
        // `commit_lock` — two committers can promote the same record at
        // once) must NOT both leave a LIVE graph node. The non-atomic
        // read-tombstone-then-reassign of the old code let both upserts
        // observe "no old internal", allocate distinct internals i1/i2,
        // insert both into the graph, and then race the final reassignment —
        // the loser's internal stayed un-tombstoned, so the rid surfaced
        // TWICE in search (and `len()` skewed) until the next rebuild-on-open.
        //
        // `entry_async` serialises the slot: the second upsert blocks on the
        // bucket entry until the first has published its internal, then sees
        // it as the "old" occupant and tombstones it. The transition
        // (read old → tombstone in `deleted` → write new internal) is done
        // entirely synchronously while the entry is held, so it is atomic
        // per rid. The CPU-bound graph insert (`spawn_blocking`) runs AFTER
        // the entry is released — we never hold the scc entry across an
        // `.await` (would violate the lock-across-await invariant), and
        // tombstoning the loser's internal does not depend on its graph
        // insert having completed: it is in `deleted` before it can ever be
        // observed live (search filters `deleted` before resolving `rid_map`).
        let internal = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut replaced: Option<usize> = None;
        {
            use scc::hash_map::Entry::{Occupied, Vacant};
            match self.rid_to_internal.entry_async(rid).await {
                Occupied(mut occ) => {
                    let old_internal = *occ.get();
                    // Tombstone the previous (or concurrently-serialised) internal.
                    if self.deleted.insert(old_internal, ()).is_ok() {
                        self.deleted_count.fetch_add(1, Ordering::Relaxed);
                        self.bump_migrated_on_tombstone(old_internal);
                    }
                    *occ.get_mut() = internal;
                    replaced = Some(old_internal);
                }
                Vacant(vac) => {
                    vac.insert_entry(internal);
                }
            }
        } // scc entry guard dropped here — NOT held across the await below.

        // V5.2 (#411) — quantized post-fit path: quantize + insert into
        // the u8 graph. Pre-fit (or unquantized) falls through to the
        // f32 path below, which also appends to the f32 buffer so the
        // fitter can snapshot it at the threshold.
        if self.quantized_active() {
            let codes = self.quantize_and_insert_u8(internal, vec).await?;
            let _ = self.vectors_u8.insert_async(internal, codes).await;
            if let Some(old) = replaced {
                let _ = self.vectors_u8.remove_async(&old).await;
            }
            let _ = self.rid_map.insert_async(internal, rid).await;
            return Ok(());
        }

        // #418 — f32 insert path. Reached ONLY when `!quantized_active()`
        // (the quantized branch above returned early). f32 graph is always
        // present in that state; None is an invariant violation → error
        // (NOT panic — upsert must propagate failure cleanly).
        let hnsw = self.hnsw.load_full().ok_or_else(|| {
            VectorError::Internal("upsert: f32 graph absent on non-quantized path".into())
        })?;
        let vec_owned = vec.to_vec();
        // Retain a copy for the exact brute-force path before the vector is
        // moved into the (CPU-bound) graph insert.
        let vec_for_store = vec_owned.clone();
        tokio::task::spawn_blocking(move || {
            hnsw.insert((&vec_owned, internal));
        })
        .await
        .map_err(|e| VectorError::Internal(e.to_string()))?;
        // `internal` is freshly allocated (monotonic `next_id`) so this never
        // collides — the insert always lands.
        let _ = self
            .vectors
            .insert_async(internal, vec_for_store.clone())
            .await;
        if let Some(old) = replaced {
            // Drop the superseded vector so brute-force never scans stale data
            // and memory stays bounded under upsert churn.
            let _ = self.vectors.remove_async(&old).await;
        }
        let _ = self.rid_map.insert_async(internal, rid).await;

        // #423 (Б-1) — self-migration re-check: if `is_fitted` flipped
        // true between our initial `quantized_active()` check and now, our
        // vector is in `vectors` but the fitter's snapshot/delta scan may
        // have already passed. We claim the codes into `vectors_u8` via
        // `claim_and_publish_u8_async`. The claim is the EXACT authority
        // for "who inserts the graph node":
        //  - If WE win the claim (Some), this internal was not yet in
        //    `vectors_u8` → the fitter will NOT insert its node (it never
        //    saw it). We must insert the u8 graph node ourselves via
        //    `quantize_and_insert_u8`.
        //  - If we LOSE the claim (None), the fitter already claimed this
        //    internal and will insert (or has inserted) its node in its
        //    final connectivity batch — we must NOT double-insert (a
        //    duplicate d_id node is a separate bug class).
        // This replaces the pre-#423 lie that "the fitter's final
        // parallel_insert handles graph connectivity" — that code did not
        // exist, so the vector was silently dropped from the graph.
        //
        if self.quantization.is_some() && self.is_fitted.load(Ordering::Acquire) {
            // #423 (Б-3, adversarial-review finding) — `!self.deleted.contains`
            // guard around the CLAIM, mirroring the three scan-based claim
            // sites (fit snapshot, delta, catch-up loop). Without it: a
            // concurrent `upsert` on the SAME rid can tombstone THIS
            // internal (via the `Occupied` rid_to_internal branch,
            // published by that call's entry_async block) between this
            // call publishing its own `internal` and reaching this check.
            // The tombstone already bumped `migrated_pre_flip` via
            // `bump_migrated_on_tombstone`; without this guard, this call
            // would ALSO claim the (now-tombstoned) internal into
            // `vectors_u8` and bump the counter a second time — an
            // over-count that can trip the catch-up loop's
            // `migrated >= target` convergence early, dropping the f32
            // graph while a genuinely still-in-flight, distinct pre-flip
            // internal has not yet landed. The buffer cleanup below still
            // runs regardless — a tombstoned internal must not linger in
            // the f32 `vectors` map (the catch-up scan and every other
            // claim path also skip `deleted` entries, so nothing else
            // would ever remove it).
            if !self.deleted.contains(&internal) {
                let codes = self.quantize(&vec_for_store);
                if self
                    .claim_and_publish_u8_async(internal, codes.clone())
                    .await
                    .is_some()
                {
                    // We own the graph node. Insert it (CPU-bound rayon
                    // insert → spawn_blocking, no guard held across the
                    // await).
                    self.insert_u8_graph_node(internal, codes).await?;
                }
            }
            // Remove from f32 buffer so the catch-up drain sees it as done
            // (whether we claimed it, lost the claim, or it was already
            // tombstoned by a concurrent racer).
            let _ = self.vectors.remove_async(&internal).await;
            return Ok(());
        }

        // V5.2 (#411) — deferred fit trigger. Only quantization-enabled
        // adapters fit, and only once (the is_fitted flag + single-flight
        // guard make this idempotent). The check is AFTER the f32 insert
        // so the just-upserted vector is in the buffer the fitter will
        // snapshot.
        if self.quantization.is_some() && !self.is_quantized() && self.len() >= FIT_THRESHOLD {
            // Best-effort: a fit failure is logged but does not fail the
            // upsert — the adapter continues on the f32 path, which is
            // always correct (just slower and 4× the memory).
            let _ = self.try_fit_and_rebuild().await;
        }
        Ok(())
    }

    /// Batch upsert with a single rayon `parallel_insert`.
    ///
    /// **Atomic dim validation:** every row's dimension is checked UP FRONT,
    /// before ANY mutation. A single mismatched row yields `Err(DimMismatch)`
    /// and leaves the graph untouched.
    ///
    /// **D12 across a batch:** we claim the rid slot per row through the same
    /// `entry_async` protocol as single `upsert` — the slot is the
    /// serialization point. Two concurrent operations (batch ↔ batch, or
    /// batch ↔ single) racing on the SAME rid both go through the rid's
    /// bucket entry: the loser observes the winner's freshly-published
    /// internal as "old" and tombstones it. Within THIS batch, a duplicate
    /// rid is handled by re-entering the same entry: the earlier row's
    /// just-published internal becomes the "old" of the later row and is
    /// tombstoned — last write wins, no orphan live node.
    ///
    /// All CPU-bound graph work (the `parallel_insert` over the collected
    /// new internals) runs in ONE `spawn_blocking` after every entry guard
    /// has been released — we never hold an scc entry across `.await`.
    async fn upsert_batch(&self, items: &[(RecordId, Vec<f32>)]) -> Result<(), VectorError> {
        if items.is_empty() {
            return Ok(());
        }
        // Atomic dim validation: fail before touching anything.
        for (_, v) in items {
            if v.len() as u32 != self.dim {
                return Err(VectorError::DimMismatch {
                    expected: self.dim,
                    got: v.len() as u32,
                });
            }
        }

        // Phase 1: per-rid slot claim (D12-safe). Collect:
        //   insert_rows: (internal, rid, owned_vec) — rows to insert into the
        //                graph; the owned Vecs move through spawn_blocking and
        //                on into `vectors` in Phase 3 (one clone total per row,
        //                matching single `upsert`).
        //   replaced    : old internals superseded by this batch (to drop
        //                from `vectors` so brute-force never scans stale data)
        let mut insert_rows: Vec<(usize, RecordId, Vec<f32>)> = Vec::with_capacity(items.len());
        let mut replaced: Vec<usize> = Vec::with_capacity(items.len());
        for (rid, vec) in items {
            let internal = self.next_id.fetch_add(1, Ordering::Relaxed);
            {
                use scc::hash_map::Entry::{Occupied, Vacant};
                match self.rid_to_internal.entry_async(*rid).await {
                    Occupied(mut occ) => {
                        let old_internal = *occ.get();
                        // Tombstone the previous (or concurrently-serialised /
                        // earlier-in-this-batch) internal. Same rationale as
                        // single `upsert`: the transition is atomic per rid
                        // while the entry is held.
                        if self.deleted.insert(old_internal, ()).is_ok() {
                            self.deleted_count.fetch_add(1, Ordering::Relaxed);
                            self.bump_migrated_on_tombstone(old_internal);
                        }
                        *occ.get_mut() = internal;
                        replaced.push(old_internal);
                    }
                    Vacant(vac) => {
                        vac.insert_entry(internal);
                    }
                }
            } // scc entry guard dropped — NOT held across the await below.
            insert_rows.push((internal, *rid, vec.clone()));
        }

        // V5.2 (#411) — quantized post-fit path: quantize the batch and
        // insert codes into the u8 graph in ONE parallel_insert. Pre-fit
        // (or unquantized) falls through to the f32 path below.
        if self.quantized_active() {
            let quantizer = self
                .quantizer
                .get()
                .expect("quantized_active but quantizer unset");
            let hnsw_u8 = self
                .hnsw_u8_handle()
                .expect("quantized_active but u8 graph unset");
            // Quantize all rows (O(dim·N) scalar — cheap vs the graph insert).
            let code_rows: Vec<(usize, RecordId, Vec<u8>)> = insert_rows
                .iter()
                .map(|(internal, rid, vec)| (*internal, *rid, quantizer.quantize(vec)))
                .collect();
            let code_rows = tokio::task::spawn_blocking(move || {
                let batch: Vec<(&Vec<u8>, usize)> =
                    code_rows.iter().map(|(i, _, c)| (c, *i)).collect();
                hnsw_u8.parallel_insert(&batch);
                code_rows
            })
            .await
            .map_err(|e| VectorError::Internal(e.to_string()))?;
            for (internal, rid, codes) in code_rows {
                let _ = self.vectors_u8.insert_async(internal, codes).await;
                let _ = self.rid_map.insert_async(internal, rid).await;
            }
            for old in replaced {
                let _ = self.vectors_u8.remove_async(&old).await;
            }
            return Ok(());
        }

        // Phase 2: ONE spawn_blocking for the whole batch — rayon
        // parallelizes the graph inserts across cores. `parallel_insert`
        // takes `&[(&Vec<T>, usize)]`; we move the OWNED rows into the
        // closure, build the borrowed slice INSIDE (so the borrows never
        // cross the `'static` boundary), and RETURN the owned rows so Phase 3
        // moves each Vec straight into `vectors` — no second clone.
        // #418 — f32 batch-insert path. Reached ONLY when `!quantized_active()`
        // (the quantized branch above returned early). f32 graph is always
        // present in that state; None is an invariant violation → error.
        let hnsw = self.hnsw.load_full().ok_or_else(|| {
            VectorError::Internal("upsert_batch: f32 graph absent on non-quantized path".into())
        })?;
        let insert_rows = tokio::task::spawn_blocking(move || {
            let batch: Vec<(&Vec<f32>, usize)> =
                insert_rows.iter().map(|(i, _rid, v)| (v, *i)).collect();
            hnsw.parallel_insert(&batch);
            insert_rows
        })
        .await
        .map_err(|e| VectorError::Internal(e.to_string()))?;

        // Phase 3: publish the per-internal bookkeeping (vectors map +
        // rid_map) and drop superseded vectors. Each map op is independent
        // and ordered so `vectors` removal of `old` cannot race a freshly
        // reused `internal` (internals are monotonic from `next_id`, so a
        // brand-new internal never aliases a tombstoned old one).
        //
        // `into_iter` moves each owned Vec straight into `vectors` — the only
        // clone of a row's vector is the Phase-1 `vec.clone()` above.
        //
        // #423 (Б-1) — self-migration re-check for the batch f32 path. If
        // `is_fitted` flipped true during this batch, the rows whose
        // `vectors.insert_async` landed AFTER the flip must migrate to
        // `vectors_u8`. Each migrated row CLAIMS its slot via
        // `claim_and_publish_u8_async`; the rows that WIN the claim owe a
        // graph node, collected into `migrated_graph_batch` for ONE
        // `parallel_insert` after the loop (mirrors the single-upsert
        // self-migration contract). Rows that LOSE the claim were already
        // claimed by the fitter, which owns their graph node.
        let mut migrated_graph_batch: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut any_migrated = false;
        for (internal, rid, vec) in insert_rows.into_iter() {
            let _ = self.vectors.insert_async(internal, vec.clone()).await;
            let _ = self.rid_map.insert_async(internal, rid).await;

            if self.quantization.is_some() && self.is_fitted.load(Ordering::Acquire) {
                any_migrated = true;
                // #423 (Б-3, adversarial-review finding) — same
                // `!self.deleted.contains` guard as the single-`upsert`
                // self-migration re-check: a concurrent `upsert`/batch on
                // the SAME rid can tombstone THIS freshly-allocated
                // `internal` between this batch publishing it (in the
                // `entry_async` loop above) and this loop reaching it. The
                // tombstone already bumped `migrated_pre_flip`; claiming a
                // tombstoned internal here would double-bump the counter.
                if !self.deleted.contains(&internal) {
                    let codes = self.quantize(&vec);
                    if self
                        .claim_and_publish_u8_async(internal, codes.clone())
                        .await
                        .is_some()
                    {
                        migrated_graph_batch.push((internal, codes));
                    }
                }
                let _ = self.vectors.remove_async(&internal).await;
            }
        }
        // #423 (Б-1) — install the graph nodes for the rows this batch
        // claimed. ONE `parallel_insert` (rayon-parallelised) for the
        // whole batch; runs only if at least one row migrated.
        if any_migrated && !migrated_graph_batch.is_empty() {
            let hnsw_u8 = self.hnsw_u8_handle().ok_or_else(|| {
                VectorError::Internal("self-migration: u8 graph not fitted".into())
            })?;
            tokio::task::spawn_blocking(move || {
                let batch: Vec<(&Vec<u8>, usize)> =
                    migrated_graph_batch.iter().map(|(i, c)| (c, *i)).collect();
                hnsw_u8.parallel_insert(&batch);
            })
            .await
            .map_err(|e| VectorError::Internal(e.to_string()))?;
        }
        for old in replaced {
            let _ = self.vectors.remove_async(&old).await;
        }

        // V5.2 (#411) — deferred fit trigger (same rationale as single
        // `upsert`). Checked AFTER the f32 inserts land in the buffer.
        if self.quantization.is_some() && !self.is_quantized() && self.len() >= FIT_THRESHOLD {
            let _ = self.try_fit_and_rebuild().await;
        }
        Ok(())
    }

    async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
        if let Some(internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
            if self.deleted.insert_async(internal, ()).await.is_ok() {
                self.deleted_count.fetch_add(1, Ordering::Relaxed);
                self.bump_migrated_on_tombstone(internal);
            }
            let _ = self.rid_to_internal.remove_async(&rid).await;
            let _ = self.vectors.remove_async(&internal).await;
            // V5.2 (#411) — also drop codes from the u8 buffer post-fit.
            // The graph node stays (hnsw_rs has no delete) but search
            // filters tombstoned internals, so it's invisible.
            if self.quantized_active() {
                let _ = self.vectors_u8.remove_async(&internal).await;
            }
        }
        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        k: u32,
        opts: SearchOpts,
        staged: Option<&[(RecordId, Vec<f32>)]>,
    ) -> Result<Vec<(RecordId, f32)>, VectorError> {
        if query.len() as u32 != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len() as u32,
            });
        }

        let k = if k == 0 {
            return Ok(vec![]);
        } else {
            k.min(MAX_TOPK)
        };

        // Per-query ef_search override (clamped to MAX_EF_SEARCH). None →
        // adapter build-time default (HnswConfig::ef_search). A clamp (not a
        // rejection) keeps untrusted input from crashing the worker — a huge
        // ef behaves like MAX_EF_SEARCH for recall but can't hold the rayon
        // pool indefinitely.
        //
        // P3 / V3.1: `opts.oversample` is consumed at the ENGINE level
        // (`read_filtered_vector_scan` requests `k′ = k × oversample`
        // candidates from this adapter, applies the residual predicate, and
        // retries with a widened `k′`). The adapter itself does NOT interpret
        // `oversample` — it returns the `k` it is asked for. We accept the
        // field so the engine can thread it through `IndexQuery::Vector`
        // without a separate channel.
        let _ = opts.oversample;
        let ef = match opts.ef_search {
            Some(v) => (v.min(MAX_EF_SEARCH) as usize).max(k as usize),
            None => self.ef_search,
        };

        // Small index → EXACT brute-force (deterministic, correct); large
        // index → approximate HNSW graph. See [`BRUTE_FORCE_MAX`].
        // Quantized brute-force threshold: 2×FIT_THRESHOLD (512). The u8
        // graph is built at FIT_THRESHOLD (256); at that scale hnsw_rs's
        // nondeterministic layer assignment can produce unreachable nodes.
        // Brute-force over 512 dim-24 codes is ~100 µs and guarantees 100%
        // recall. Beyond 512 the graph is large enough for reliable
        // connectivity.
        const QUANT_BRUTE_FORCE_MAX: usize = FIT_THRESHOLD * 2;
        let mut results: Vec<(RecordId, f32)> =
            if self.quantized_active() && self.len() <= QUANT_BRUTE_FORCE_MAX {
                // V5.2 (#411) — small quantized index: exact brute-force scan
                // over dequantized vectors (same as unquantized brute-force but
                // reading from vectors_u8 via the quantizer). Guarantees 100%
                // recall on small indexes where the HNSW graph may have poor
                // connectivity due to hnsw_rs's nondeterministic layer assignment.
                //
                // #424 (Б-4) — body extracted to [`search_quantized_bruteforce`]
                // so the transient-None retry path in the f32-graph branch below
                // can re-run the EXACT same logic instead of returning empty.
                self.search_quantized_bruteforce(query, k).await?
            } else if self.quantized_active() {
                // V5.2 (#411) — post-fit quantized search: traversal on the u8
                // graph (cheap integer distance) with overscan `16k+64`, then
                // dequant-rescore each candidate with the EXACT f32 distance to
                // the original (unquantized) query.
                //
                // #424 (Б-4) — body extracted to [`search_quantized_graph`] so
                // the transient-None retry path in the f32-graph branch below
                // can re-run the EXACT same logic instead of returning empty.
                self.search_quantized_graph(query, k, ef).await?
            } else if self.len() <= BRUTE_FORCE_MAX {
                let dist = ShamirDist {
                    metric: self.metric,
                };
                // Snapshot (internal, vector) pairs — the index is tiny here.
                let mut pairs: Vec<(usize, Vec<f32>)> = Vec::with_capacity(128);
                self.vectors.scan(|i, v| pairs.push((*i, v.clone())));
                let mut out: Vec<(RecordId, f32)> = Vec::with_capacity(pairs.len());
                for (internal, v) in pairs {
                    if self.deleted.contains_async(&internal).await {
                        continue;
                    }
                    if let Some(rid) = self.rid_map.read_async(&internal, |_, r| *r).await {
                        out.push((rid, dist.eval(query, &v)));
                    }
                }
                out
            } else {
                // Search committed f32 graph (approximate). Reached ONLY when
                // `!quantized_active()` AND `len() > BRUTE_FORCE_MAX` — in that
                // state the f32 graph is always present... UNLESS a concurrent
                // fit transition raced between this function's `quantized_active()`
                // gate (above) and the `hnsw.load_full()` below.
                //
                // #424 (Б-4) — transient-None race window. The sequence:
                //   1. THIS request read `quantized_active() == false` (pre-flip)
                //      in the `if/else if` chain above, so it landed here on the
                //      f32 path.
                //   2. A concurrent fitter ran `try_fit_and_rebuild`: it set
                //      `is_fitted = true` (Release) and then `hnsw.store(None)`
                //      (Release) to drop the f32 graph (#418 memory win).
                //   3. THIS request's `hnsw.load_full()` now returns `None`.
                //
                // The old code called this "invariant violation, unreachable" and
                // returned `Ok(vec![])` — but the window is REAL (the gate and the
                // load are two independent atomic reads with no lock between them).
                // A request that hits it silently got an empty result instead of a
                // correct answer through the now-live u8 graph.
                //
                // Fix: on `None`, RE-CHECK `quantized_active()`. If it is now
                // `true`, the flip happened in the window — route through the u8
                // path (same helper the main quantized branch uses). If it is
                // STILL `false`, the f32 graph is genuinely absent on a
                // non-quantized adapter — a true invariant violation (unquantized
                // adapters never drop the f32 graph; quantized adapters only drop
                // it post-flip, which IS `quantized_active()`) — defensive empty.
                //
                // #424 (Б-4) — test hook: pause HERE (after the
                // `quantized_active() == false` gate, before `load_full()`)
                // so a deterministic test can trigger a fit transition that
                // drops the f32 graph while this request is paused. Compiles
                // away in release builds.
                #[cfg(test)]
                if let Some(gate) = TEST_SEARCH_F32_GATE.load_full() {
                    // Signal the test that this request has reached the
                    // f32-gate (past the `quantized_active() == false`
                    // check, now paused before `load_full()`).
                    gate.arrived
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    // `notified()` is cancellation-safe: if the test has
                    // already called `notify_one` before we get here, the
                    // permit is stored and `notified()` resolves immediately.
                    gate.notify.notified().await;
                }
                let hnsw = match self.hnsw.load_full() {
                    Some(h) => h,
                    None => {
                        // #424 (Б-4) — transient flip during this request. Re-check
                        // and route through the now-correct u8 path.
                        if self.quantized_active() {
                            if self.len() <= QUANT_BRUTE_FORCE_MAX {
                                return self.search_quantized_bruteforce(query, k).await;
                            } else {
                                return self.search_quantized_graph(query, k, ef).await;
                            }
                        }
                        // Genuinely unreachable: a non-quantized adapter (or a
                        // pre-flip quantized adapter) has no reason for the f32
                        // graph to be absent. Defensive empty on a read path —
                        // never panic.
                        return Ok(vec![]);
                    }
                };
                let overscan = (k as usize) * 2 + 10;
                let query_owned = query.to_vec();
                let neighbors =
                    tokio::task::spawn_blocking(move || hnsw.search(&query_owned, overscan, ef))
                        .await
                        .map_err(|e| VectorError::Internal(e.to_string()))?;

                let mut out: Vec<(RecordId, f32)> = Vec::with_capacity(k as usize + 16);
                for n in neighbors {
                    if self.deleted.contains_async(&n.d_id).await {
                        continue;
                    }
                    if let Some(rid) = self.rid_map.read_async(&n.d_id, |_, v| *v).await {
                        out.push((rid, n.distance));
                    }
                }
                out
            };

        // Merge the caller's own un-committed staged vectors (in-tx search)
        // via a brute-force scan — they are not in the committed graph.
        if let Some(staged) = staged {
            let dist = ShamirDist {
                metric: self.metric,
            };
            for (rid, vec) in staged {
                let d = dist.eval(query, vec);
                results.push((*rid, d));
            }
        }

        // Sort by distance ascending, truncate to k.
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k as usize);

        Ok(results)
    }

    fn dim(&self) -> u32 {
        self.dim
    }

    fn len(&self) -> usize {
        // `saturating_sub`: two independent Relaxed loads; a stale `next_id`
        // with a fresher `deleted_count` under concurrent replace/delete
        // would underflow a plain subtraction (see `live_count`).
        self.next_id
            .load(Ordering::Relaxed)
            .saturating_sub(self.deleted_count.load(Ordering::Relaxed))
    }

    /// V2.3 (#402) — `HnswAdapter` IS the snapshot-able adapter. The
    /// background snapshot trigger in `VectorBackend` uses this to recover
    /// the concrete type for `dump_snapshot_with_gen`.
    fn as_hnsw_adapter(&self) -> Option<&HnswAdapter> {
        Some(self)
    }
}
