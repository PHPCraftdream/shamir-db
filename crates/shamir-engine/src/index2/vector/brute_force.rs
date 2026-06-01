//! Brute-force exact KNN adapter.
//!
//! O(n) per query — correct baseline. Lock-free read / single-writer
//! write: writes go through a bounded mpsc, a spawned task drains the
//! channel and applies ops to its owned working state, then publishes
//! a fresh `Arc<BruteSnap>` via `ArcSwap`. Readers grab the current
//! snapshot with one atomic load — no locks.
//!
//! Layout: snapshot is a parallel-array struct
//! (`rids` / `vecs` / `norms`) plus a `HashMap<RecordId, usize>` index
//! for O(1) upsert/delete. Norms are precomputed at insert so cosine
//! search doesn't re-scan the stored vector on every distance call.
//!
//! Write coalescing: the actor task drains every queued op via
//! `try_recv` before publishing, so a burst of M upserts pays ONE
//! `Arc<BruteSnap>` clone instead of M (the original
//! `(**snap.load()).clone()`-per-op pattern was O(M·N)).
//!
//! Search: top-k is computed with a bounded `BinaryHeap` of size k
//! (O(N log k)) instead of a full sort over all N entries (O(N log N)).

use super::adapter::{VectorAdapter, VectorError};
use super::simd::{dot_product, l2_squared};
use crate::index2::kind::VectorMetric;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use shamir_types::types::record_id::RecordId;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Parallel-array snapshot. Lock-free read: a search loads the `Arc`
/// once and scans the three slices.
#[derive(Default)]
struct BruteSnap {
    rids: Vec<RecordId>,
    vecs: Vec<Vec<f32>>,
    /// Precomputed L2 norm of each stored vector (used only by Cosine;
    /// kept unconditionally so the snapshot layout is metric-agnostic
    /// and the per-search hot loop has no metric-specific branches on
    /// the stored side).
    norms: Vec<f32>,
    /// `rid -> position in the parallel arrays`. Maintained by the
    /// writer task so upsert / delete are O(1) lookups.
    index: HashMap<RecordId, usize>,
}

enum WriteOp {
    Upsert(RecordId, Vec<f32>),
    Delete(RecordId),
}

pub struct BruteForceAdapter {
    dim: u32,
    metric: VectorMetric,
    write_tx: mpsc::Sender<WriteOp>,
    snapshot: Arc<ArcSwap<BruteSnap>>,
    join: std::sync::Mutex<Option<JoinHandle<()>>>,
}

/// Default bounded-channel capacity (mirrors `IndexActor`'s default).
/// Bounded so a runaway producer can't OOM the server; large enough
/// to absorb bulk-import bursts and let the writer task coalesce
/// many ops into a single publish.
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

impl BruteForceAdapter {
    pub fn new(dim: u32, metric: VectorMetric) -> Self {
        let snapshot = Arc::new(ArcSwap::from(Arc::new(BruteSnap::default())));
        let (write_tx, mut rx) = mpsc::channel::<WriteOp>(DEFAULT_CHANNEL_CAPACITY);
        let snap_for_task = snapshot.clone();

        let join = tokio::spawn(async move {
            // Working state owned by this task — never shared, so we
            // mutate in place and publish a fresh `Arc<BruteSnap>`
            // once per drained batch (NOT once per op).
            let mut work = BruteSnap::default();
            while let Some(first) = rx.recv().await {
                apply_op(&mut work, first);
                // Coalesce: drain everything else already queued so
                // M ops cost ONE publish (one Arc allocation + one
                // O(N) clone of the snapshot arrays).
                while let Ok(op) = rx.try_recv() {
                    apply_op(&mut work, op);
                }
                snap_for_task.store(Arc::new(clone_snap(&work)));
            }
        });

        Self {
            dim,
            metric,
            write_tx,
            snapshot,
            join: std::sync::Mutex::new(Some(join)),
        }
    }

    /// Distance with the stored vector's norm provided by the caller
    /// (precomputed at insert time). Only the query-side norm is
    /// computed per search, and only for `Cosine`.
    #[inline]
    fn distance_with_stored_norm(
        metric: VectorMetric,
        query: &[f32],
        query_norm: f32,
        stored: &[f32],
        stored_norm: f32,
    ) -> f32 {
        match metric {
            VectorMetric::L2 => l2_squared(query, stored).sqrt(),
            VectorMetric::Cosine => {
                if query_norm < 1e-9 || stored_norm < 1e-9 {
                    return 1.0;
                }
                let dot = dot_product(query, stored);
                1.0 - dot / (query_norm * stored_norm)
            }
            VectorMetric::Dot => -dot_product(query, stored),
        }
    }

    pub async fn shutdown(self) {
        drop(self.write_tx);
        let join = self.join.lock().expect("brute-force join lock").take();
        if let Some(j) = join {
            let _ = j.await;
        }
    }
}

#[inline]
fn norm_of(v: &[f32]) -> f32 {
    dot_product(v, v).sqrt()
}

fn apply_op(work: &mut BruteSnap, op: WriteOp) {
    match op {
        WriteOp::Upsert(rid, vec) => {
            let n = norm_of(&vec);
            if let Some(&pos) = work.index.get(&rid) {
                work.vecs[pos] = vec;
                work.norms[pos] = n;
            } else {
                let pos = work.rids.len();
                work.rids.push(rid);
                work.vecs.push(vec);
                work.norms.push(n);
                work.index.insert(rid, pos);
            }
        }
        WriteOp::Delete(rid) => {
            if let Some(pos) = work.index.remove(&rid) {
                // swap_remove keeps O(1) but renames the last entry's
                // index slot.
                let last = work.rids.len() - 1;
                work.rids.swap_remove(pos);
                work.vecs.swap_remove(pos);
                work.norms.swap_remove(pos);
                if pos != last {
                    let moved_rid = work.rids[pos];
                    work.index.insert(moved_rid, pos);
                }
            }
        }
    }
}

fn clone_snap(src: &BruteSnap) -> BruteSnap {
    BruteSnap {
        rids: src.rids.clone(),
        vecs: src.vecs.clone(),
        norms: src.norms.clone(),
        index: src.index.clone(),
    }
}

/// Bounded-heap top-K helper. We keep the K WORST so far at the top
/// (`Reverse`-style max-heap by distance); a new candidate with
/// smaller distance pops the worst and pushes itself, so the heap
/// always holds the current K best. Final drain + sort is O(K log K).
fn push_topk(heap: &mut BinaryHeap<HeapEntry>, k: usize, rid: RecordId, dist: f32) {
    if k == 0 {
        return;
    }
    if heap.len() < k {
        heap.push(HeapEntry { rid, dist });
    } else if let Some(top) = heap.peek() {
        // BinaryHeap is a max-heap on `Ord`; HeapEntry orders by dist
        // ascending semantics inverted below (so peek = worst). Swap
        // if the candidate beats the current worst.
        if dist < top.dist {
            heap.pop();
            heap.push(HeapEntry { rid, dist });
        }
    }
}

/// Max-heap entry: larger `dist` is "greater" so `peek()` returns the
/// current worst candidate in the top-K window.
#[derive(Debug)]
struct HeapEntry {
    rid: RecordId,
    dist: f32,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.dist == other.dist
    }
}
impl Eq for HeapEntry {}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Treat NaN as Equal (matches the old `partial_cmp(...).unwrap_or(Equal)`).
        self.dist
            .partial_cmp(&other.dist)
            .unwrap_or(Ordering::Equal)
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
        self.write_tx
            .send(WriteOp::Upsert(rid, vec.to_vec()))
            .await
            .map_err(|_| VectorError::Internal("actor stopped".into()))?;
        // Yield to let the actor make progress — not guaranteed but
        // helps single-threaded tests observe the write soon. Kept
        // for behavioural parity with the previous impl.
        tokio::task::yield_now().await;
        Ok(())
    }

    async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
        self.write_tx
            .send(WriteOp::Delete(rid))
            .await
            .map_err(|_| VectorError::Internal("actor stopped".into()))?;
        tokio::task::yield_now().await;
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
        let snap = self.snapshot.load_full();
        let k_usize = k as usize;
        let metric = self.metric;
        let query_norm = match metric {
            VectorMetric::Cosine => norm_of(query),
            _ => 0.0,
        };

        let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::with_capacity(k_usize.saturating_add(1));

        // Committed snapshot.
        for i in 0..snap.rids.len() {
            let d = Self::distance_with_stored_norm(
                metric,
                query,
                query_norm,
                &snap.vecs[i],
                snap.norms[i],
            );
            push_topk(&mut heap, k_usize, snap.rids[i], d);
        }

        // Staged (in-tx) vectors — caller's own un-committed writes.
        if let Some(staged) = staged {
            for (rid, vec) in staged {
                let stored_norm = match metric {
                    VectorMetric::Cosine => norm_of(vec),
                    _ => 0.0,
                };
                let d =
                    Self::distance_with_stored_norm(metric, query, query_norm, vec, stored_norm);
                push_topk(&mut heap, k_usize, *rid, d);
            }
        }

        // Drain heap → ascending distance order.
        let mut out: Vec<(RecordId, f32)> = heap.into_iter().map(|e| (e.rid, e.dist)).collect();
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        Ok(out)
    }

    fn dim(&self) -> u32 {
        self.dim
    }

    fn len(&self) -> usize {
        self.snapshot.load().rids.len()
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

        let results = adapter.search(&[1.0, 0.0, 0.0], 2, None).await.unwrap();
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

        let results = adapter.search(&[0.0, 0.0], 2, None).await.unwrap();
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
        let results = adapter.search(&[1.0, 0.0], 2, None).await.unwrap();
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

        let results = adapter.search(&[0.0, 0.0], 10, None).await.unwrap();
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
        let results = adapter.search(&[10.0, 10.0], 1, None).await.unwrap();
        assert_eq!(results[0].0, rid(1));
        assert!(results[0].1 < 0.01);
    }
}
