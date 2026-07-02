//! HNSW approximate nearest neighbor adapter using `hnsw_rs`.
//!
//! `hnsw_rs::Hnsw` is internally thread-safe (RwLock per layer for
//! insert, lock-free traversal for search). We wrap it directly —
//! no actor needed for HNSW itself.
//!
//! Deletion via soft-delete tombstone set; search over-scans ×2 to
//! compensate for filtered-out tombstones.

use super::adapter::{VectorAdapter, VectorError};
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
const MAX_TOPK: u32 = 10_000;

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
            next_id: AtomicUsize::new(0),
        }
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
                    let _ = self.deleted.insert(old_internal, ());
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

    async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
        if let Some(internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
            let _ = self.deleted.insert_async(internal, ()).await;
            let _ = self.rid_to_internal.remove_async(&rid).await;
            let _ = self.vectors.remove_async(&internal).await;
        }
        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        k: u32,
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
            let ef = self.ef_search;
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

    #[allow(clippy::disallowed_methods)] // O(N) ack: deleted-tombstone count for live cardinality, off hot path
    fn len(&self) -> usize {
        self.next_id.load(Ordering::Relaxed) - self.deleted.len()
    }
}
