use super::config::{HnswConfig, ShamirDist};
use crate::kind::{VectorMetric, VectorQuantization};
use crate::vector::quantized_dist::ShamirDistU8;
use crate::vector::sq8::Sq8Quantizer;
use arc_swap::ArcSwapOption;
use hnsw_rs::hnsw::Hnsw;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

pub struct HnswAdapter {
    pub(super) dim: u32,
    pub(super) metric: VectorMetric,
    pub(super) ef_search: usize,
    /// Opt-in quantization mode. `None` → the adapter never builds a u8
    /// graph and the entire pipeline is the legacy f32 path, bit-for-bit.
    pub(super) quantization: Option<VectorQuantization>,
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
    pub(super) hnsw: ArcSwapOption<Hnsw<'static, f32, ShamirDist>>,
    pub(crate) rid_map: scc::HashMap<usize, RecordId, THasher>,
    pub(super) rid_to_internal: scc::HashMap<RecordId, usize, THasher>,
    /// Raw vectors retained (keyed by internal id) so a small index can be
    /// searched EXACTLY by brute force — see [`BRUTE_FORCE_MAX`]. Tombstoned
    /// entries are removed here on replace/delete.
    ///
    /// In the quantized path this map holds f32 vectors ONLY pre-fit; once
    /// [`Self::is_fitted`] flips true the post-fit inserts go to
    /// [`Self::vectors_u8`] and this map is drained.
    pub(super) vectors: scc::HashMap<usize, Vec<f32>, THasher>,
    pub(crate) deleted: scc::HashMap<usize, (), THasher>,
    pub(super) deleted_count: AtomicUsize,
    pub(super) next_id: AtomicUsize,
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
    pub(super) hnsw_u8: arc_swap::ArcSwapOption<Hnsw<'static, u8, ShamirDistU8>>,
    /// SQ8 codes keyed by internal id. Populated at fit and on every
    /// post-fit insert. The u8 graph ALSO stores its own `Vec<u8>` per
    /// DataPoint, so this map is technically redundant with the graph's
    /// internal copy — BUT the graph does not expose a `get_vector(id)`
    /// accessor, so we keep the authoritative codes here for rescore and
    /// `collect_live_vectors`. This is the 4×-memory win: u8 codes (dim
    /// bytes) vs f32 vectors (4·dim bytes).
    pub(super) vectors_u8: scc::HashMap<usize, Vec<u8>, THasher>,
    /// Frozen quantizer params. `None` until fit; `Some(Arc::new(q))`
    /// after fit (read-only for the lifetime of the adapter).
    pub(super) quantizer: std::sync::OnceLock<Arc<Sq8Quantizer>>,
    /// Publish flag: `true` once the u8 graph is live. See the struct doc
    /// for the memory-ordering argument.
    pub(super) is_fitted: AtomicBool,
    /// Single-flight guard: the FIRST caller to CAS this `true` performs
    /// the fit+rebuild; every other concurrent caller skips straight to
    /// the f32 path (their internals are re-inserted by the fitter's
    /// delta pass after the swap).
    pub(super) fit_in_flight: AtomicBool,
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
    pub(super) migrated_pre_flip: AtomicUsize,
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
    pub(super) next_id_at_flip: AtomicUsize,
}

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
        self.rid_map.iter_sync(|internal, rid| {
            f(*internal, *rid);
            true
        });
    }

    /// Iterate `(rid -> internal)` pairs for snapshot serialisation.
    pub(crate) fn for_each_rid_to_internal<F: FnMut(RecordId, usize)>(&self, mut f: F) {
        self.rid_to_internal.iter_sync(|rid, internal| {
            f(*rid, *internal);
            true
        });
    }

    /// Check if a rid exists in the live index (not tombstoned).
    #[allow(dead_code)] // API for #408 compaction tests
    pub(crate) fn contains_rid(&self, rid: &RecordId) -> bool {
        self.rid_to_internal.contains_sync(rid)
    }

    /// Iterate the tombstone (`deleted`) internals for snapshot serialisation.
    pub(crate) fn for_each_deleted<F: FnMut(usize)>(&self, mut f: F) {
        self.deleted.iter_sync(|internal, ()| {
            f(*internal);
            true
        });
    }

    /// Iterate `(internal -> vector)` pairs for snapshot serialisation.
    pub(crate) fn for_each_vector<F: FnMut(usize, &[f32])>(&self, mut f: F) {
        self.vectors.iter_sync(|internal, vec| {
            f(*internal, vec);
            true
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
}
