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
    pub(crate) adapter: Arc<dyn VectorAdapter>,
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
