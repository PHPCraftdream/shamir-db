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
use crate::index2::kind::VectorMetric;
use async_trait::async_trait;
use hnsw_rs::anndists::dist::distances::Distance;
use hnsw_rs::hnsw::Hnsw;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct ShamirDist {
    metric: VectorMetric,
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
    rid_map: scc::HashMap<usize, RecordId>,
    rid_to_internal: scc::HashMap<RecordId, usize>,
    deleted: scc::HashMap<usize, ()>,
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
        Self {
            dim,
            metric,
            ef_search: config.ef_search,
            hnsw: Arc::new(hnsw),
            rid_map: scc::HashMap::new(),
            rid_to_internal: scc::HashMap::new(),
            deleted: scc::HashMap::new(),
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
        {
            use scc::hash_map::Entry::{Occupied, Vacant};
            match self.rid_to_internal.entry_async(rid).await {
                Occupied(mut occ) => {
                    let old_internal = *occ.get();
                    // Tombstone the previous (or concurrently-serialised) internal.
                    let _ = self.deleted.insert(old_internal, ());
                    *occ.get_mut() = internal;
                }
                Vacant(vac) => {
                    vac.insert_entry(internal);
                }
            }
        } // scc entry guard dropped here — NOT held across the await below.

        let hnsw = Arc::clone(&self.hnsw);
        let vec_owned = vec.to_vec();
        tokio::task::spawn_blocking(move || {
            hnsw.insert((&vec_owned, internal));
        })
        .await
        .map_err(|e| VectorError::Internal(e.to_string()))?;
        let _ = self.rid_map.insert_async(internal, rid).await;
        Ok(())
    }

    async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
        if let Some(internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
            let _ = self.deleted.insert_async(internal, ()).await;
            let _ = self.rid_to_internal.remove_async(&rid).await;
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

        // Search committed graph
        let hnsw = Arc::clone(&self.hnsw);
        let ef = self.ef_search;
        let overscan = (k as usize) * 2 + 10;
        let query_owned = query.to_vec();
        let neighbors =
            tokio::task::spawn_blocking(move || hnsw.search(&query_owned, overscan, ef))
                .await
                .map_err(|e| VectorError::Internal(e.to_string()))?;

        let mut results: Vec<(RecordId, f32)> = Vec::with_capacity(k as usize + 16);
        for n in neighbors {
            if self.deleted.contains_async(&n.d_id).await {
                continue;
            }
            if let Some(rid) = self.rid_map.read_async(&n.d_id, |_, v| *v).await {
                results.push((rid, n.distance));
            }
        }

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
        self.next_id.load(Ordering::Relaxed) - self.deleted.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(n: u8) -> RecordId {
        let mut a = [0u8; 16];
        a[15] = n;
        RecordId(a)
    }

    fn random_vec(dim: usize, seed: u64) -> Vec<f32> {
        let mut v = Vec::with_capacity(dim);
        let mut s = seed;
        for _ in 0..dim {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            v.push(((s >> 33) as f32) / (u32::MAX as f32) - 0.5);
        }
        v
    }

    #[tokio::test]
    async fn basic_cosine_search() {
        let adapter = HnswAdapter::new(
            3,
            VectorMetric::Cosine,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );

        adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[0.0, 1.0, 0.0]).await.unwrap();
        adapter.upsert(rid(3), &[0.9, 0.1, 0.0]).await.unwrap();

        let results = adapter.search(&[1.0, 0.0, 0.0], 2, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, rid(1));
        assert!(results[0].1 < 0.01);
    }

    #[tokio::test]
    async fn delete_removes_from_results() {
        // Deletion correctness must not hinge on HNSW recall. On a tiny
        // 2-node graph, soft-deleting the entry point makes search
        // intermittently return 0 results (recall artifact on a
        // degenerate graph — not a soft-delete bug). To assert behaviour
        // deterministically we:
        //   1. build a non-degenerate graph (10 points) so recall over
        //      the survivors is reliable;
        //   2. assert the deleted rid is ABSENT (the actual contract of
        //      delete — never relies on recall reaching one survivor);
        //   3. assert the survivors ARE found (recall sanity on a graph
        //      large enough that it holds).
        // Mirrors the presence/absence pattern of
        // `apply_committed_vectors_inserts_all_into_graph` and
        // `recall_at_10_on_1k_vectors`.
        let adapter = HnswAdapter::new(
            2,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );

        // 10 points spread along x so neighbours are well separated.
        for i in 0..10u8 {
            adapter.upsert(rid(i), &[i as f32, 0.0]).await.unwrap();
        }

        adapter.delete(rid(0)).await.unwrap();

        let results = adapter.search(&[0.0, 0.0], 10, None).await.unwrap();

        // Contract: the deleted rid must never surface.
        assert!(
            !results.iter().any(|(r, _)| *r == rid(0)),
            "deleted rid must be absent; got {results:?}"
        );
        // Recall sanity on a non-degenerate graph: the surviving nearest
        // neighbour (rid 1 at [1,0]) is found.
        assert!(
            results.iter().any(|(r, _)| *r == rid(1)),
            "surviving nearest neighbour must be found; got {results:?}"
        );
    }

    #[tokio::test]
    async fn upsert_replaces() {
        let adapter = HnswAdapter::new(
            2,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );

        adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(1), &[10.0, 10.0]).await.unwrap();

        let results = adapter.search(&[10.0, 10.0], 1, None).await.unwrap();
        assert_eq!(results[0].0, rid(1));
        assert!(results[0].1 < 0.01);
    }

    #[tokio::test]
    async fn recall_at_10_on_1k_vectors() {
        // `hnsw_rs` 0.3.4 has no public seed API — `Hnsw::new` calls
        // `StdRng::from_os_rng()` internally, so the graph topology is
        // non-deterministic between runs. To keep this test stable while
        // still exercising HNSW under realistic load we:
        //   1. raise ef_search well above the dataset's natural variance
        //      (a search that visits ~half the graph hits recall 1.0
        //      almost always);
        //   2. require recall ≥ 0.5 — a soft floor that catches gross
        //      regressions (broken Distance impl, broken pruning) without
        //      flaking on the ~5% of runs where the random graph is
        //      adversarial.
        // Tighter recall validation belongs in a separate bench-only run.
        let dim = 32;
        let n = 1000;
        let adapter = HnswAdapter::new(
            dim as u32,
            VectorMetric::L2,
            HnswConfig {
                max_elements: n + 100,
                ef_construction: 400,
                ef_search: 400,
                ..Default::default()
            },
        );

        let mut vecs = Vec::with_capacity(n);
        for i in 0..n {
            let v = random_vec(dim, i as u64 + 42);
            adapter.upsert(rid(0), &v).await.unwrap();
            // Use unique rids:
            let mut a = [0u8; 16];
            a[14] = (i >> 8) as u8;
            a[15] = (i & 0xFF) as u8;
            let r = RecordId(a);
            adapter.upsert(r, &v).await.unwrap();
            vecs.push((r, v));
        }

        // Brute-force ground truth for query = vecs[0].
        let query = &vecs[0].1;
        let mut dists: Vec<(RecordId, f32)> = vecs
            .iter()
            .map(|(r, v)| {
                let d: f32 = query
                    .iter()
                    .zip(v.iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f32>()
                    .sqrt();
                (*r, d)
            })
            .collect();
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let gt_top10: std::collections::HashSet<RecordId> =
            dists.iter().take(10).map(|(r, _)| *r).collect();

        let hnsw_results = adapter.search(query, 10, None).await.unwrap();
        let hnsw_top10: std::collections::HashSet<RecordId> =
            hnsw_results.iter().map(|(r, _)| *r).collect();

        let recall = gt_top10.intersection(&hnsw_top10).count() as f64 / 10.0;
        assert!(recall >= 0.5, "recall@10 = {recall:.2} — expected >= 0.50");
    }

    #[tokio::test]
    async fn dim_mismatch_rejected() {
        let adapter = HnswAdapter::new(3, VectorMetric::L2, HnswConfig::default());
        let err = adapter.upsert(rid(1), &[1.0, 2.0]).await.unwrap_err();
        assert!(matches!(
            err,
            VectorError::DimMismatch {
                expected: 3,
                got: 2
            }
        ));
    }

    #[tokio::test]
    async fn search_dim_mismatch_rejected() {
        let adapter = HnswAdapter::new(3, VectorMetric::L2, HnswConfig::default());
        adapter.upsert(rid(1), &[1.0, 2.0, 3.0]).await.unwrap();
        let err = adapter.search(&[1.0, 2.0], 1, None).await.unwrap_err();
        assert!(matches!(err, VectorError::DimMismatch { .. }));
    }

    #[tokio::test]
    async fn empty_index_returns_empty() {
        let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
        let results = adapter.search(&[0.0, 0.0], 5, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn dot_product_metric_normalized() {
        // Direct test of `ShamirDist` evaluator for the Dot metric — three
        // hand-picked normalized vectors, exact distance values, exact
        // ordering. We sidestep HNSW here because the graph is non-
        // deterministic (no seed API in hnsw_rs 0.3.4) and unstable
        // ordering would force soft assertions even at n=3. The HNSW
        // integration is covered by `basic_cosine_search` and `recall_at_10`.
        let dist = ShamirDist {
            metric: VectorMetric::Dot,
        };
        let s = 2.0_f32.sqrt().recip();
        let q = [1.0_f32, 0.0];

        // dot(q, [1,0]) = 1.0  → dist 0.0
        // dot(q, [s,s]) ≈ 0.707 → dist 0.293
        // dot(q, [0,1]) = 0.0  → dist 1.0
        let d_self = dist.eval(&q, &[1.0, 0.0]);
        let d_diag = dist.eval(&q, &[s, s]);
        let d_orth = dist.eval(&q, &[0.0, 1.0]);

        assert!(d_self < 0.01, "self-similarity should be ~0, got {d_self}");
        assert!(
            (d_diag - (1.0 - s)).abs() < 0.01,
            "diag dist should be ~{}, got {d_diag}",
            1.0 - s
        );
        assert!(
            (d_orth - 1.0).abs() < 0.01,
            "orthogonal dist should be ~1.0, got {d_orth}"
        );
        // Ordering invariant: nearer < farther.
        assert!(d_self < d_diag && d_diag < d_orth);
    }

    #[tokio::test]
    async fn k_larger_than_dataset() {
        let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
        adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[1.0, 0.0]).await.unwrap();

        let results = adapter.search(&[0.0, 0.0], 100, None).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn concurrent_searches_lock_free() {
        let dim = 16;
        let adapter = std::sync::Arc::new(HnswAdapter::new(
            dim,
            VectorMetric::Cosine,
            HnswConfig {
                max_elements: 1000,
                ..Default::default()
            },
        ));
        // Populate
        for i in 0..100 {
            let mut a = [0u8; 16];
            a[15] = i as u8;
            adapter
                .upsert(RecordId(a), &random_vec(dim as usize, i as u64))
                .await
                .unwrap();
        }

        // 8 concurrent searches.
        let mut handles = Vec::new();
        for s in 0..8u64 {
            let a = std::sync::Arc::clone(&adapter);
            handles.push(tokio::spawn(async move {
                let q = random_vec(dim as usize, s + 100);
                a.search(&q, 10, None).await.unwrap()
            }));
        }
        for h in handles {
            let r = h.await.unwrap();
            assert!(!r.is_empty());
        }
    }

    #[tokio::test]
    async fn delete_nonexistent_no_error() {
        let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
        adapter.delete(rid(99)).await.unwrap();
    }

    #[tokio::test]
    async fn search_merges_staged_slice() {
        // The committed graph holds rid(1); the caller passes its own
        // un-committed vector (rid(2), very close to the query) as a
        // staged slice. `search` must brute-force-merge it so an in-tx
        // query sees both. This is the path the executor drives from
        // `TxContext::staged_vectors_for(token)`.
        let adapter = HnswAdapter::new(
            3,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );
        adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();

        let staged = vec![(rid(2), vec![0.9, 0.1, 0.0])];

        // Without the staged slice: only the committed vector.
        let committed_only = adapter.search(&[1.0, 0.0, 0.0], 10, None).await.unwrap();
        let committed_rids: std::collections::HashSet<_> =
            committed_only.iter().map(|(r, _)| *r).collect();
        assert!(committed_rids.contains(&rid(1)));
        assert!(
            !committed_rids.contains(&rid(2)),
            "staged vector must be invisible without the slice"
        );

        // With the staged slice: both surface.
        let merged = adapter
            .search(&[1.0, 0.0, 0.0], 10, Some(&staged))
            .await
            .unwrap();
        let merged_rids: std::collections::HashSet<_> = merged.iter().map(|(r, _)| *r).collect();
        assert!(
            merged_rids.contains(&rid(1)),
            "committed vector still found"
        );
        assert!(merged_rids.contains(&rid(2)), "staged vector merged in");
    }

    #[tokio::test]
    async fn apply_committed_vectors_inserts_all_into_graph() {
        let adapter = HnswAdapter::new(
            3,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );

        // Before apply — search finds nothing.
        let before = adapter.search(&[1.0, 0.0, 0.0], 10, None).await.unwrap();
        assert_eq!(before.len(), 0, "graph empty before apply");

        // Apply a committed batch (what commit Phase 5d feeds in).
        let batch = vec![
            (rid(1), vec![1.0, 0.0, 0.0]),
            (rid(2), vec![0.0, 1.0, 0.0]),
            (rid(3), vec![0.0, 0.0, 1.0]),
        ];
        adapter.apply_committed_vectors(&batch).await.unwrap();

        // After apply — the closest vector (rid 1 = the query) is findable.
        let after = adapter.search(&[1.0, 0.0, 0.0], 10, None).await.unwrap();
        assert!(
            !after.is_empty(),
            "graph must hold committed vectors after apply"
        );
        let labels: std::collections::HashSet<_> = after.iter().map(|(r, _)| *r).collect();
        assert!(
            labels.contains(&rid(1)),
            "closest applied vector (rid 1) must be findable; got {after:?}"
        );
    }

    #[tokio::test]
    async fn apply_committed_vectors_empty_is_noop() {
        let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
        adapter.apply_committed_vectors(&[]).await.unwrap(); // must not panic
    }

    #[tokio::test]
    async fn apply_committed_vectors_handles_replace() {
        let adapter = HnswAdapter::new(
            3,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );

        // A committed vector at [1,0,0].
        adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
        let before = adapter.search(&[1.0, 0.0, 0.0], 1, None).await.unwrap();
        assert_eq!(before[0].0, rid(1));

        // Apply a committed batch that replaces rid(1) with a new vector.
        adapter
            .apply_committed_vectors(&[(rid(1), vec![0.0, 1.0, 0.0])])
            .await
            .unwrap();

        // Search for [0,1,0] -> should find rid(1) (updated position).
        let after = adapter.search(&[0.0, 1.0, 0.0], 1, None).await.unwrap();
        assert_eq!(after[0].0, rid(1));
        assert!(after[0].1 < 0.01, "should be very close to [0,1,0]");
    }

    #[tokio::test]
    async fn many_upserts_same_rid() {
        let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
        for i in 0..10 {
            adapter.upsert(rid(1), &[i as f32, 0.0]).await.unwrap();
        }
        // Only latest visible
        let results = adapter.search(&[9.0, 0.0], 10, None).await.unwrap();
        let matching: Vec<_> = results.iter().filter(|(r, _)| *r == rid(1)).collect();
        assert_eq!(
            matching.len(),
            1,
            "rid(1) should appear once after 10 upserts"
        );
        assert!(matching[0].1 < 0.5);
    }

    /// D12: two concurrent `upsert(SAME rid, different vec)` must leave the
    /// rid mapped to EXACTLY ONE live internal — never two. This is the race
    /// reachable since III.5 moved HNSW promote outside `commit_lock` (two
    /// committers promoting the same record concurrently).
    ///
    /// Against the OLD non-atomic read-tombstone-reassign, both upserts read
    /// "no old internal", allocate i1/i2, insert both into the graph, and the
    /// loser's internal stays un-tombstoned — so the rid surfaces twice in
    /// search and `len()` over-counts. The `entry_async` slot-claim fixes it:
    /// the second upsert serialises on the bucket entry, sees the first's
    /// internal as "old", and tombstones it.
    ///
    /// We drive many barrier-synced iterations to surface the interleaving,
    /// and assert the invariant two ways: (a) directly on adapter state — the
    /// count of live (non-tombstoned) internals mapped to the rid is exactly
    /// 1, which is recall-independent and the decisive old-vs-new
    /// discriminator; and (b) end-to-end via search on a non-degenerate graph
    /// — the rid appears at most once.
    #[tokio::test]
    #[serial_test::serial]
    async fn upsert_same_rid_concurrent_no_duplicate() {
        use std::sync::Arc as StdArc;
        use tokio::sync::Barrier;

        let dim = 4usize;
        // Each iteration is a fresh adapter so the race window is clean and
        // any single failure is decisive. Enough iterations to reliably
        // surface the interleaving on the racy code.
        for iter in 0..40u64 {
            let adapter = StdArc::new(HnswAdapter::new(
                dim as u32,
                VectorMetric::L2,
                HnswConfig {
                    max_elements: 1000,
                    ..Default::default()
                },
            ));

            // Populate a non-degenerate graph with unrelated rids so search
            // recall over survivors is reliable (the same rationale as
            // `delete_removes_from_results`). rid bytes start at 10 so they
            // never collide with the contended rid (1).
            for j in 0..12u8 {
                adapter
                    .upsert(rid(10 + j), &random_vec(dim, iter * 100 + j as u64))
                    .await
                    .unwrap();
            }

            let target = rid(1);
            let vec_a = vec![1.0f32, 0.0, 0.0, 0.0];
            let vec_b = vec![0.0f32, 1.0, 0.0, 0.0];

            // Two tasks race an upsert of the SAME rid, synced at the barrier
            // so both enter the critical section as close together as the
            // runtime allows.
            let barrier = StdArc::new(Barrier::new(2));
            let mut handles = Vec::new();
            for v in [vec_a.clone(), vec_b.clone()] {
                let a = StdArc::clone(&adapter);
                let b = StdArc::clone(&barrier);
                handles.push(tokio::spawn(async move {
                    b.wait().await;
                    a.upsert(target, &v).await.unwrap();
                }));
            }
            for h in handles {
                h.await.unwrap();
            }

            // (a) Direct state invariant (recall-independent): exactly one
            //     internal mapped to `target` is live (present in rid_map and
            //     NOT tombstoned). The old racy path leaves two.
            let mut live_internals = 0usize;
            adapter
                .rid_map
                .scan_async(|internal, mapped_rid| {
                    if *mapped_rid == target && !adapter.deleted.contains(internal) {
                        live_internals += 1;
                    }
                })
                .await;
            assert_eq!(
                live_internals, 1,
                "iter {iter}: rid must map to exactly ONE live internal after \
                 two concurrent upserts; found {live_internals} (D12 duplicate-rid race)"
            );

            // (b) End-to-end: the rid appears AT MOST once in a top-k search
            //     near either staged vector. (Recall may legitimately miss it
            //     on a tiny random graph, but it must never appear twice.)
            for q in [&vec_a, &vec_b] {
                let results = adapter.search(q, 16, None).await.unwrap();
                let occurrences = results.iter().filter(|(r, _)| *r == target).count();
                assert!(
                    occurrences <= 1,
                    "iter {iter}: rid surfaced {occurrences} times in search — \
                     duplicate live graph node (D12); results={results:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn non_tx_search_unchanged_after_refactor() {
        // Regression guard: existing non-tx path works exactly as before.
        let adapter = HnswAdapter::new(
            3,
            VectorMetric::Cosine,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );
        adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[0.0, 1.0, 0.0]).await.unwrap();
        adapter.upsert(rid(3), &[0.9, 0.1, 0.0]).await.unwrap();

        let results = adapter.search(&[1.0, 0.0, 0.0], 2, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, rid(1));
    }
}
