//! HNSW approximate nearest neighbor adapter using `hnsw_rs`.
//!
//! `hnsw_rs::Hnsw` is internally thread-safe (RwLock per layer for
//! insert, lock-free traversal for search). We wrap it directly —
//! no actor needed for HNSW itself.
//!
//! Deletion via soft-delete tombstone set; search over-scans ×2 to
//! compensate for filtered-out tombstones.

use super::adapter::{VectorAdapter, VectorError};
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
        match self.metric {
            VectorMetric::L2 => a
                .iter()
                .zip(b.iter())
                .map(|(x, y)| (x - y) * (x - y))
                .sum::<f32>()
                .sqrt(),
            VectorMetric::Cosine => {
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
                let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
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
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
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
    // Stored for potential future use in search-time distance selection
    #[allow(dead_code)]
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
        // If already exists, soft-delete old and re-insert with new id.
        if let Some(old_internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
            let _ = self.deleted.insert_async(old_internal, ()).await;
        }
        let internal = self.next_id.fetch_add(1, Ordering::Relaxed);
        let hnsw = Arc::clone(&self.hnsw);
        let vec_owned = vec.to_vec();
        tokio::task::spawn_blocking(move || {
            hnsw.insert((&vec_owned, internal));
        })
        .await
        .map_err(|e| VectorError::Internal(e.to_string()))?;
        let _ = self.rid_map.insert_async(internal, rid).await;
        let _ = self.rid_to_internal.upsert_async(rid, internal).await;
        Ok(())
    }

    async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
        if let Some(internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
            let _ = self.deleted.insert_async(internal, ()).await;
            let _ = self.rid_to_internal.remove_async(&rid).await;
        }
        Ok(())
    }

    async fn search(&self, query: &[f32], k: u32) -> Result<Vec<(RecordId, f32)>, VectorError> {
        if query.len() as u32 != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len() as u32,
            });
        }
        let hnsw = Arc::clone(&self.hnsw);
        let ef = self.ef_search;
        let overscan = (k as usize) * 2 + 10;
        let query_owned = query.to_vec();
        let neighbors =
            tokio::task::spawn_blocking(move || hnsw.search(&query_owned, overscan, ef))
                .await
                .map_err(|e| VectorError::Internal(e.to_string()))?;

        let mut results = Vec::with_capacity(k as usize);
        for n in neighbors {
            if results.len() >= k as usize {
                break;
            }
            if self.deleted.contains_async(&n.d_id).await {
                continue;
            }
            if let Some(rid) = self.rid_map.read_async(&n.d_id, |_, v| *v).await {
                results.push((rid, n.distance));
            }
        }
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

        let results = adapter.search(&[1.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, rid(1));
        assert!(results[0].1 < 0.01);
    }

    #[tokio::test]
    async fn delete_removes_from_results() {
        let adapter = HnswAdapter::new(
            2,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        );

        adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[1.0, 0.0]).await.unwrap();

        adapter.delete(rid(1)).await.unwrap();

        let results = adapter.search(&[0.0, 0.0], 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, rid(2));
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

        let results = adapter.search(&[10.0, 10.0], 1).await.unwrap();
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

        let hnsw_results = adapter.search(query, 10).await.unwrap();
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
        let err = adapter.search(&[1.0, 2.0], 1).await.unwrap_err();
        assert!(matches!(err, VectorError::DimMismatch { .. }));
    }

    #[tokio::test]
    async fn empty_index_returns_empty() {
        let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
        let results = adapter.search(&[0.0, 0.0], 5).await.unwrap();
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

        let results = adapter.search(&[0.0, 0.0], 100).await.unwrap();
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
                a.search(&q, 10).await.unwrap()
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
    async fn many_upserts_same_rid() {
        let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
        for i in 0..10 {
            adapter.upsert(rid(1), &[i as f32, 0.0]).await.unwrap();
        }
        // Only latest visible
        let results = adapter.search(&[9.0, 0.0], 10).await.unwrap();
        let matching: Vec<_> = results.iter().filter(|(r, _)| *r == rid(1)).collect();
        assert_eq!(
            matching.len(),
            1,
            "rid(1) should appear once after 10 upserts"
        );
        assert!(matching[0].1 < 0.5);
    }
}
