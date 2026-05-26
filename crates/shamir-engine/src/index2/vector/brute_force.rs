//! Brute-force exact KNN adapter.
//!
//! O(n) per query — correct baseline. Uses `IndexActor` for lock-free
//! read/write: writes go through mpsc, reads grab an `ArcSwap`
//! snapshot of the vector store (a plain `Vec<(RecordId, Vec<f32>)>`).
//!
//! Swap in HNSW as the snapshot type for approximate search later.

use super::adapter::{VectorAdapter, VectorError};
use crate::index2::actor::IndexActor;
use crate::index2::kind::VectorMetric;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use shamir_types::types::record_id::RecordId;
use std::collections::HashMap;
use std::sync::Arc;

type Snapshot = HashMap<RecordId, Vec<f32>>;

enum WriteOp {
    Upsert(RecordId, Vec<f32>),
    Delete(RecordId),
}

pub struct BruteForceAdapter {
    dim: u32,
    metric: VectorMetric,
    actor: IndexActor<WriteOp, Snapshot>,
}

impl BruteForceAdapter {
    pub fn new(dim: u32, metric: VectorMetric) -> Self {
        let actor = IndexActor::spawn(
            HashMap::new(),
            |op, snap: Arc<ArcSwap<Snapshot>>| async move {
                let mut current: Snapshot = (**snap.load()).clone();
                match op {
                    WriteOp::Upsert(rid, vec) => {
                        current.insert(rid, vec);
                    }
                    WriteOp::Delete(rid) => {
                        current.remove(&rid);
                    }
                }
                snap.store(Arc::new(current));
            },
        );
        Self { dim, metric, actor }
    }

    fn distance(metric: &VectorMetric, a: &[f32], b: &[f32]) -> f32 {
        match metric {
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
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                -dot // negate so lower = better (consistent with distance)
            }
        }
    }

    pub async fn shutdown(self) {
        self.actor.shutdown().await;
    }
}

#[async_trait]
impl VectorAdapter for BruteForceAdapter {
    async fn upsert(&self, rid: RecordId, vec: &[f32]) -> Result<(), VectorError> {
        if vec.len() as u32 != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: vec.len() as u32,
            });
        }
        self.actor
            .submit(WriteOp::Upsert(rid, vec.to_vec()))
            .map_err(|_| VectorError::Internal("actor stopped".into()))?;
        // Yield to let the actor process — not guaranteed but helps tests.
        tokio::task::yield_now().await;
        Ok(())
    }

    async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
        self.actor
            .submit(WriteOp::Delete(rid))
            .map_err(|_| VectorError::Internal("actor stopped".into()))?;
        tokio::task::yield_now().await;
        Ok(())
    }

    async fn search(&self, query: &[f32], k: u32) -> Result<Vec<(RecordId, f32)>, VectorError> {
        if query.len() as u32 != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len() as u32,
            });
        }
        let snap = self.actor.snapshot();
        let mut dists: Vec<(RecordId, f32)> = snap
            .iter()
            .map(|(rid, vec)| (*rid, Self::distance(&self.metric, query, vec)))
            .collect();
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        dists.truncate(k as usize);
        Ok(dists)
    }

    fn dim(&self) -> u32 {
        self.dim
    }

    fn len(&self) -> usize {
        self.actor.snapshot().len()
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

    #[tokio::test]
    async fn cosine_basic() {
        let adapter = BruteForceAdapter::new(3, VectorMetric::Cosine);
        adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[0.0, 1.0, 0.0]).await.unwrap();
        adapter.upsert(rid(3), &[1.0, 1.0, 0.0]).await.unwrap();

        // Wait for actor to process writes.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let results = adapter.search(&[1.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, rid(1)); // exact match = distance 0
        assert!(results[0].1 < 0.01);
    }

    #[tokio::test]
    async fn l2_basic() {
        let adapter = BruteForceAdapter::new(2, VectorMetric::L2);
        adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[3.0, 4.0]).await.unwrap();
        adapter.upsert(rid(3), &[1.0, 0.0]).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let results = adapter.search(&[0.0, 0.0], 2).await.unwrap();
        assert_eq!(results[0].0, rid(1)); // distance 0
        assert_eq!(results[1].0, rid(3)); // distance 1
    }

    #[tokio::test]
    async fn dot_product() {
        let adapter = BruteForceAdapter::new(2, VectorMetric::Dot);
        adapter.upsert(rid(1), &[1.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[0.5, 0.5]).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // query = [1, 0], dot with rid(1)=1.0, dot with rid(2)=0.5
        // negated: rid(1)=-1.0 < rid(2)=-0.5 → rid(1) first
        let results = adapter.search(&[1.0, 0.0], 2).await.unwrap();
        assert_eq!(results[0].0, rid(1));
    }

    #[tokio::test]
    async fn delete_removes_from_search() {
        let adapter = BruteForceAdapter::new(2, VectorMetric::L2);
        adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
        adapter.upsert(rid(2), &[1.0, 0.0]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        adapter.delete(rid(1)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let results = adapter.search(&[0.0, 0.0], 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, rid(2));
    }

    #[tokio::test]
    async fn dim_mismatch_rejected() {
        let adapter = BruteForceAdapter::new(3, VectorMetric::L2);
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
    async fn upsert_replaces() {
        let adapter = BruteForceAdapter::new(2, VectorMetric::L2);
        adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        adapter.upsert(rid(1), &[10.0, 10.0]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        assert_eq!(adapter.len(), 1);
        let results = adapter.search(&[10.0, 10.0], 1).await.unwrap();
        assert_eq!(results[0].0, rid(1));
        assert!(results[0].1 < 0.01);
    }
}
