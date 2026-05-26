//! `IndexBackend` wrapper around any `VectorAdapter`.
//!
//! Extracts the vector field from records, delegates to the adapter
//! for similarity search. Returns `IndexResult::Ranked`.

use super::adapter::{VectorAdapter, VectorError};
use crate::index2::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::index2::descriptor::IndexDescriptor;
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

    async fn on_insert(&self, rid: RecordId, rec: &InnerValue) -> Result<(), IndexError> {
        if let Some(v) = self.extract_vec(rec) {
            self.adapter.upsert(rid, &v).await.map_err(ve)?;
        }
        Ok(())
    }

    async fn on_update(
        &self,
        rid: RecordId,
        _old: &InnerValue,
        new: &InnerValue,
    ) -> Result<(), IndexError> {
        if let Some(v) = self.extract_vec(new) {
            self.adapter.upsert(rid, &v).await.map_err(ve)?;
        } else {
            self.adapter.delete(rid).await.map_err(ve)?;
        }
        Ok(())
    }

    async fn on_delete(&self, rid: RecordId, _rec: &InnerValue) -> Result<(), IndexError> {
        self.adapter.delete(rid).await.map_err(ve)?;
        Ok(())
    }

    async fn on_batch_insert(
        &self,
        items: &[(RecordId, &InnerValue)],
    ) -> Result<(), IndexError> {
        for (rid, rec) in items {
            self.on_insert(*rid, rec).await?;
        }
        Ok(())
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Vector { vec, k } => {
                let results = self.adapter.search(&vec, k).await.map_err(ve)?;
                Ok(IndexResult::Ranked(results))
            }
            _ => Err(IndexError::Backend(
                "VectorBackend only supports Vector queries".into(),
            )),
        }
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
    use bytes::Bytes;
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
            .on_insert(r1, &make_rec(&i, &[1.0, 0.0, 0.0]))
            .await
            .unwrap();
        backend
            .on_insert(r2, &make_rec(&i, &[0.0, 1.0, 0.0]))
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
        backend.on_insert(r1, &rec).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        backend.on_delete(r1, &rec).await.unwrap();
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
    async fn rebuild_from_store() {
        use shamir_storage::storage_in_memory::InMemoryStore;
        use crate::index2::backend::IndexBackend;

        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

        // Write 3 records into the store as (RecordId → InnerValue).
        let r1 = RecordId::new();
        let r2 = RecordId::new();
        let r3 = RecordId::new();
        let rec1 = make_rec(&i, &[1.0, 0.0, 0.0]);
        let rec2 = make_rec(&i, &[0.0, 1.0, 0.0]);
        let rec3 = make_rec(&i, &[0.9, 0.1, 0.0]);
        store.set(r1.to_bytes(), Bytes::from(rec1.to_bytes().unwrap())).await.unwrap();
        store.set(r2.to_bytes(), Bytes::from(rec2.to_bytes().unwrap())).await.unwrap();
        store.set(r3.to_bytes(), Bytes::from(rec3.to_bytes().unwrap())).await.unwrap();

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
