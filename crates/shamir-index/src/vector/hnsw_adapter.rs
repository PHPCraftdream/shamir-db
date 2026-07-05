//! HNSW approximate nearest neighbor adapter using `hnsw_rs`.
//!
//! `hnsw_rs::Hnsw` is internally thread-safe (RwLock per layer for
//! insert, lock-free traversal for search). We wrap it directly —
//! no actor needed for HNSW itself.
//!
//! Deletion via soft-delete tombstone set; search over-scans ×2 to
//! compensate for filtered-out tombstones.

use super::adapter::{SearchOpts, VectorAdapter, VectorError};
use super::simd::{dot_product, l2_squared};
use crate::kind::VectorMetric;
use async_trait::async_trait;
use hnsw_rs::anndists::dist::distances::Distance;
use hnsw_rs::hnsw::Hnsw;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicUsize, Ordering};
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
                1.0 - dot / (na * nb)
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
    hnsw: Arc<Hnsw<'static, f32, ShamirDist>>,
    pub(crate) rid_map: scc::HashMap<usize, RecordId, THasher>,
    rid_to_internal: scc::HashMap<RecordId, usize, THasher>,
    /// Raw vectors retained (keyed by internal id) so a small index can be
    /// searched EXACTLY by brute force — see [`BRUTE_FORCE_MAX`]. Tombstoned
    /// entries are removed here on replace/delete.
    vectors: scc::HashMap<usize, Vec<f32>, THasher>,
    pub(crate) deleted: scc::HashMap<usize, (), THasher>,
    deleted_count: AtomicUsize,
    next_id: AtomicUsize,
}

impl HnswAdapter {
    pub fn new(dim: u32, metric: VectorMetric, config: HnswConfig) -> Self {
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
            hnsw: Arc::new(hnsw),
            rid_map: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            rid_to_internal: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            vectors: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            deleted: scc::HashMap::with_capacity_and_hasher(cap, THasher::default()),
            deleted_count: AtomicUsize::new(0),
            next_id: AtomicUsize::new(0),
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

    pub(crate) fn hnsw_handle(&self) -> &Arc<Hnsw<'static, f32, ShamirDist>> {
        &self.hnsw
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
            hnsw,
            rid_map,
            rid_to_internal,
            vectors,
            deleted,
            deleted_count: AtomicUsize::new(deleted_cnt),
            next_id: AtomicUsize::new(next_id),
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
    #[allow(dead_code)] // API for #408 compaction trigger
    pub(crate) fn deleted_ratio(&self) -> f64 {
        let next = self.next_id.load(Ordering::Relaxed);
        if next == 0 {
            return 0.0;
        }
        self.deleted_count.load(Ordering::Relaxed) as f64 / next as f64
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

        let mut scored: Vec<(RecordId, f32)> = Vec::with_capacity(candidates.len());
        for &rid in candidates {
            let internal = match self.rid_to_internal.read_async(&rid, |_, v| *v).await {
                Some(i) => i,
                None => continue,
            };
            if self.deleted.contains_async(&internal).await {
                continue;
            }
            let vec_opt = self.vectors.read_async(&internal, |_, v| v.clone()).await;
            if let Some(v) = vec_opt {
                let d = dist.eval(query, &v);
                scored.push((rid, d));
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

        let hnsw = Arc::clone(&self.hnsw);
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
                    }
                    *occ.get_mut() = internal;
                    replaced = Some(old_internal);
                }
                Vacant(vac) => {
                    vac.insert_entry(internal);
                }
            }
        } // scc entry guard dropped here — NOT held across the await below.

        let hnsw = Arc::clone(&self.hnsw);
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
        let _ = self.vectors.insert_async(internal, vec_for_store).await;
        if let Some(old) = replaced {
            // Drop the superseded vector so brute-force never scans stale data
            // and memory stays bounded under upsert churn.
            let _ = self.vectors.remove_async(&old).await;
        }
        let _ = self.rid_map.insert_async(internal, rid).await;
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

        // Phase 2: ONE spawn_blocking for the whole batch — rayon
        // parallelizes the graph inserts across cores. `parallel_insert`
        // takes `&[(&Vec<T>, usize)]`; we move the OWNED rows into the
        // closure, build the borrowed slice INSIDE (so the borrows never
        // cross the `'static` boundary), and RETURN the owned rows so Phase 3
        // moves each Vec straight into `vectors` — no second clone.
        let hnsw = Arc::clone(&self.hnsw);
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
        for (internal, rid, vec) in insert_rows.into_iter() {
            let _ = self.vectors.insert_async(internal, vec).await;
            let _ = self.rid_map.insert_async(internal, rid).await;
        }
        for old in replaced {
            let _ = self.vectors.remove_async(&old).await;
        }
        Ok(())
    }

    async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
        if let Some(internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
            if self.deleted.insert_async(internal, ()).await.is_ok() {
                self.deleted_count.fetch_add(1, Ordering::Relaxed);
            }
            let _ = self.rid_to_internal.remove_async(&rid).await;
            let _ = self.vectors.remove_async(&internal).await;
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
        let mut results: Vec<(RecordId, f32)> = if self.len() <= BRUTE_FORCE_MAX {
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
            // Search committed graph (approximate).
            let hnsw = Arc::clone(&self.hnsw);
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
