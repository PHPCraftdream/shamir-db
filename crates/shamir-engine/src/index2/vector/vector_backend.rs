//! `IndexBackend` wrapper around any `VectorAdapter`.
//!
//! Extracts the vector field from records, delegates to the adapter
//! for similarity search. Returns `IndexResult::Ranked`.

use super::adapter::{VectorAdapter, VectorError};
use crate::index2::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::write_ops::IndexWriteOp;
use async_trait::async_trait;
use futures::StreamExt;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

pub struct VectorBackend {
    descriptor: IndexDescriptor,
    field_path: Vec<u64>,
    adapter: Arc<dyn VectorAdapter>,
}

impl VectorBackend {
    pub fn new(
        descriptor: IndexDescriptor,
        field_path: Vec<u64>,
        adapter: Arc<dyn VectorAdapter>,
    ) -> Self {
        Self {
            descriptor,
            field_path,
            adapter,
        }
    }

    fn extract_vec(&self, rec: &InnerValue) -> Option<Vec<f32>> {
        let mut current = rec;
        for &seg in &self.field_path {
            match current {
                InnerValue::Map(m) => {
                    let key = shamir_types::core::interner::InternerKey::new(seg);
                    current = m.get(&key)?;
                }
                _ => return None,
            }
        }
        match current {
            InnerValue::List(items) => {
                let mut v = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        InnerValue::F64(f) => v.push(*f as f32),
                        InnerValue::Int(n) => v.push(*n as f32),
                        _ => return None,
                    }
                }
                Some(v)
            }
            _ => None,
        }
    }
}

fn ve(e: VectorError) -> IndexError {
    IndexError::Backend(e.to_string())
}

#[async_trait]
impl IndexBackend for VectorBackend {
    fn descriptor(&self) -> &IndexDescriptor {
        &self.descriptor
    }

    // HNSW mutation goes through the adapter. The non-tx variants
    // (`plan_insert` / `plan_update` / `plan_delete`) commit to the
    // live HNSW graph immediately. The tx-aware overrides below
    // (`plan_insert_tx` / `plan_update_tx` / `plan_delete_tx`) do NOT
    // touch the live graph when `tx_id == Some`: the executor instead
    // extracts the vector via [`staged_vector`] and buffers it in
    // `TxContext::staged_vectors`, so a rolled-back tx leaves no ghost
    // vectors on the live graph (HIGH-6).
    //
    // HIGH-6 (resolved, fully RAII): staged vectors live inside the
    // `TxContext` (tx-local state), not on the adapter. The commit
    // pipeline (`commit::commit_tx` Phase 5d) calls
    // `apply_staged_vectors` under the commit lock to promote the tx's
    // staged vectors into the live graph. Abort needs no counterpart —
    // a dropped tx discards `staged_vectors` by RAII. Non-tx queries
    // still see only the committed graph (see `HnswAdapter::search`).

    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if let Some(v) = self.extract_vec(rec) {
            self.adapter.upsert(rid, &v).await.map_err(ve)?;
        }
        Ok(Vec::new())
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        _old: &InnerValue,
        new: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if let Some(v) = self.extract_vec(new) {
            self.adapter.upsert(rid, &v).await.map_err(ve)?;
        } else {
            self.adapter.delete(rid).await.map_err(ve)?;
        }
        Ok(Vec::new())
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        _rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        self.adapter.delete(rid).await.map_err(ve)?;
        Ok(Vec::new())
    }

    /// tx-aware insert planning (HIGH-6).
    ///
    /// `tx_id == Some` → no-op here: the live HNSW graph is left
    /// untouched. The executor stages the vector itself via
    /// [`staged_vector`] → `TxContext::stage_vector`, so a dropped
    /// (rolled-back) tx leaves no trace (RAII).
    ///
    /// `tx_id == None` → forwards to [`plan_insert`] (immediate
    /// commit to the live graph).
    async fn plan_insert_tx(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if tx_id.is_none() {
            return self.plan_insert(rid, rec).await;
        }
        Ok(Vec::new())
    }

    /// tx-aware update planning (HIGH-6). See [`plan_insert_tx`].
    async fn plan_update_tx(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if tx_id.is_none() {
            return self.plan_update(rid, old, new).await;
        }
        Ok(Vec::new())
    }

    /// tx-aware delete planning (HIGH-6). See [`plan_insert_tx`].
    async fn plan_delete_tx(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if tx_id.is_none() {
            return self.plan_delete(rid, rec).await;
        }
        Ok(Vec::new())
    }

    /// HIGH-6: hand the executor this record's embedding so it can stage
    /// it tx-locally (`TxContext::stage_vector`). `None` when the record
    /// carries no vector at this backend's field path.
    async fn staged_vector(&self, _rid: RecordId, rec: &InnerValue) -> Option<Vec<f32>> {
        self.extract_vec(rec)
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Vector { vec, k } => {
                let results = self.adapter.search(&vec, k, None).await.map_err(ve)?;
                Ok(IndexResult::Ranked(results))
            }
            _ => Err(IndexError::Backend(
                "VectorBackend only supports Vector queries".into(),
            )),
        }
    }

    async fn lookup_tx(
        &self,
        _table_token: u64,
        query: IndexQuery,
        _tx: Option<&shamir_tx::TxContext>,
        staged_vectors: Option<&[(RecordId, Vec<f32>)]>,
    ) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Vector { vec, k } => {
                let results = self
                    .adapter
                    .search(&vec, k, staged_vectors)
                    .await
                    .map_err(ve)?;
                Ok(IndexResult::Ranked(results))
            }
            _ => Err(IndexError::Backend(
                "VectorBackend only supports Vector queries".into(),
            )),
        }
    }

    /// HIGH-6: promote the tx's staged vectors for this table into the
    /// live HNSW graph at commit. Delegates to the adapter.
    async fn apply_staged_vectors(&self, vecs: &[(RecordId, Vec<f32>)]) -> Result<(), IndexError> {
        self.adapter.apply_committed_vectors(vecs).await.map_err(ve)
    }

    async fn rebuild(&self, source: Arc<dyn Store>) -> Result<(), IndexError> {
        let batch_size = 1000usize;
        let mut stream = source.iter_stream(batch_size);
        while let Some(batch_res) = stream.next().await {
            let batch = batch_res.map_err(|e| IndexError::Storage(e.to_string()))?;
            let mut items: Vec<(RecordId, Vec<f32>)> = Vec::new();
            for (key_bytes, val_bytes) in batch {
                let arr: [u8; 16] = key_bytes
                    .as_ref()
                    .try_into()
                    .map_err(|_| IndexError::Backend("invalid key length".into()))?;
                let rid = RecordId(arr);
                let rec = InnerValue::from_bytes(&val_bytes)
                    .map_err(|e| IndexError::Backend(e.to_string()))?;
                if let Some(v) = self.extract_vec(&rec) {
                    items.push((rid, v));
                }
            }
            for chunk in items.chunks(64) {
                for (rid, v) in chunk {
                    self.adapter.upsert(*rid, v).await.map_err(ve)?;
                }
            }
        }
        Ok(())
    }

    async fn drop_all(&self) -> Result<(), IndexError> {
        // Adapter doesn't have a "clear" method — for now noop.
        // Full impl would iterate all rids and delete.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index2::kind::{IndexKind, VectorBackendRef, VectorConfig, VectorMetric};
    use crate::index2::vector::brute_force::BruteForceAdapter;
    use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
    use shamir_types::types::common::new_map_wc;
    use smallvec::SmallVec;

    fn intern(i: &Interner, s: &str) -> u64 {
        match i.touch_ind(s).unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    }

    fn make_rec(interner: &Interner, embedding: &[f64]) -> InnerValue {
        let mut m = new_map_wc(1);
        m.insert(
            InternerKey::new(intern(interner, "embedding")),
            InnerValue::List(embedding.iter().map(|f| InnerValue::F64(*f)).collect()),
        );
        InnerValue::Map(m)
    }

    fn make_backend(interner: &Interner) -> VectorBackend {
        let desc = IndexDescriptor::new(
            30,
            "vec_idx",
            intern(interner, "vec_idx"),
            SmallVec::new(),
            IndexKind::Vector(Box::new(VectorConfig {
                dim: 3,
                metric: VectorMetric::Cosine,
                backend: VectorBackendRef::InProcessHnsw {
                    ef_construct: 200,
                    m: 16,
                },
            })),
        );
        let adapter = Arc::new(BruteForceAdapter::new(3, VectorMetric::Cosine));
        VectorBackend::new(desc, vec![intern(interner, "embedding")], adapter)
    }

    #[tokio::test]
    async fn insert_and_search() {
        let i = Interner::new();
        let backend = make_backend(&i);

        let r1 = RecordId::new();
        let r2 = RecordId::new();
        backend
            .plan_insert(r1, &make_rec(&i, &[1.0, 0.0, 0.0]))
            .await
            .unwrap();
        backend
            .plan_insert(r2, &make_rec(&i, &[0.0, 1.0, 0.0]))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let result = backend
            .lookup(IndexQuery::Vector {
                vec: vec![1.0, 0.0, 0.0],
                k: 1,
            })
            .await
            .unwrap();

        match result {
            IndexResult::Ranked(ranked) => {
                assert_eq!(ranked.len(), 1);
                assert_eq!(ranked[0].0, r1);
            }
            _ => panic!("expected Ranked"),
        }
    }

    #[tokio::test]
    async fn delete_excludes_from_search() {
        let i = Interner::new();
        let backend = make_backend(&i);

        let r1 = RecordId::new();
        let rec = make_rec(&i, &[1.0, 0.0, 0.0]);
        backend.plan_insert(r1, &rec).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        backend.plan_delete(r1, &rec).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let result = backend
            .lookup(IndexQuery::Vector {
                vec: vec![1.0, 0.0, 0.0],
                k: 10,
            })
            .await
            .unwrap();

        match result {
            IndexResult::Ranked(ranked) => assert!(ranked.is_empty()),
            _ => panic!("expected Ranked"),
        }
    }

    #[tokio::test]
    async fn lookup_tx_none_matches_lookup() {
        let i = Interner::new();
        let backend = make_backend(&i);

        let r1 = RecordId::new();
        let r2 = RecordId::new();
        backend
            .plan_insert(r1, &make_rec(&i, &[1.0, 0.0, 0.0]))
            .await
            .unwrap();
        backend
            .plan_insert(r2, &make_rec(&i, &[0.0, 1.0, 0.0]))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let q = IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
        };
        let via_lookup = backend
            .lookup(IndexQuery::Vector {
                vec: vec![1.0, 0.0, 0.0],
                k: 2,
            })
            .await
            .unwrap();
        let via_tx = backend.lookup_tx(0, q, None, None).await.unwrap();
        match (via_lookup, via_tx) {
            (IndexResult::Ranked(a), IndexResult::Ranked(b)) => {
                assert_eq!(a.len(), b.len());
            }
            _ => panic!("expected Ranked results"),
        }
    }

    #[tokio::test]
    async fn lookup_tx_some_includes_staged_vector() {
        use crate::index2::vector::hnsw_adapter::{HnswAdapter, HnswConfig};

        let i = Interner::new();

        let desc = IndexDescriptor::new(
            31,
            "vec_tx",
            intern(&i, "vec_tx"),
            SmallVec::new(),
            IndexKind::Vector(Box::new(VectorConfig {
                dim: 3,
                metric: VectorMetric::Cosine,
                backend: VectorBackendRef::InProcessHnsw {
                    ef_construct: 200,
                    m: 16,
                },
            })),
        );
        let adapter: Arc<dyn VectorAdapter> = Arc::new(HnswAdapter::new(
            3,
            VectorMetric::Cosine,
            HnswConfig {
                max_elements: 100,
                ..Default::default()
            },
        ));
        let backend = VectorBackend::new(desc, vec![intern(&i, "embedding")], adapter);

        // Commit one vector via non-tx upsert.
        let committed_rid = RecordId::new();
        backend
            .adapter
            .upsert(committed_rid, &[1.0, 0.0, 0.0])
            .await
            .unwrap();

        // The tx's own staged vector (what the executor buffers in
        // `TxContext::staged_vectors_for(token)`), very close to query.
        let staged_rid = RecordId::new();
        let staged: Vec<(RecordId, Vec<f32>)> = vec![(staged_rid, vec![0.9, 0.1, 0.0])];

        // Non-tx lookup sees only committed vector.
        let non_tx = backend
            .lookup(IndexQuery::Vector {
                vec: vec![1.0, 0.0, 0.0],
                k: 2,
            })
            .await
            .unwrap();

        // tx-aware lookup gets the staged slice threaded in by the caller.
        let in_tx = backend
            .lookup_tx(
                0,
                IndexQuery::Vector {
                    vec: vec![1.0, 0.0, 0.0],
                    k: 2,
                },
                None,
                Some(&staged),
            )
            .await
            .unwrap();

        let non_tx_rids: Vec<RecordId> = match non_tx {
            IndexResult::Ranked(r) => r.into_iter().map(|(rid, _)| rid).collect(),
            _ => panic!("expected Ranked"),
        };
        let in_tx_rids: Vec<RecordId> = match in_tx {
            IndexResult::Ranked(r) => r.into_iter().map(|(rid, _)| rid).collect(),
            _ => panic!("expected Ranked"),
        };

        assert!(
            !non_tx_rids.contains(&staged_rid),
            "non-tx must not see staged vector"
        );
        assert!(
            in_tx_rids.contains(&staged_rid),
            "in-tx lookup must merge staged vector: got {in_tx_rids:?}"
        );
        assert!(
            non_tx_rids.contains(&committed_rid),
            "non-tx must see committed vector"
        );
        assert!(
            in_tx_rids.contains(&committed_rid),
            "in-tx must see committed vector"
        );
    }

    #[tokio::test]
    async fn rebuild_from_store() {
        use crate::index2::backend::IndexBackend;
        use shamir_storage::storage_in_memory::InMemoryStore;

        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

        // Write 3 records into the store as (RecordId → InnerValue).
        let r1 = RecordId::new();
        let r2 = RecordId::new();
        let r3 = RecordId::new();
        let rec1 = make_rec(&i, &[1.0, 0.0, 0.0]);
        let rec2 = make_rec(&i, &[0.0, 1.0, 0.0]);
        let rec3 = make_rec(&i, &[0.9, 0.1, 0.0]);
        store
            .set(r1.to_bytes(), rec1.to_bytes().unwrap())
            .await
            .unwrap();
        store
            .set(r2.to_bytes(), rec2.to_bytes().unwrap())
            .await
            .unwrap();
        store
            .set(r3.to_bytes(), rec3.to_bytes().unwrap())
            .await
            .unwrap();

        // Create a fresh backend (empty adapter) and rebuild from store.
        let backend = make_backend(&i);
        backend.rebuild(Arc::clone(&store)).await.unwrap();

        // Search for [1,0,0] — top-2 should contain r1 (closest).
        let result = backend
            .lookup(IndexQuery::Vector {
                vec: vec![1.0, 0.0, 0.0],
                k: 2,
            })
            .await
            .unwrap();
        match result {
            IndexResult::Ranked(ranked) => {
                assert_eq!(ranked.len(), 2, "expected 2 results, got {ranked:?}");
                assert_eq!(ranked[0].0, r1, "r1 should be the closest");
            }
            _ => panic!("expected Ranked"),
        }
    }
}
