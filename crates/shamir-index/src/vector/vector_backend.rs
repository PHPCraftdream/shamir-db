//! `IndexBackend` wrapper around any `VectorAdapter`.
//!
//! Extracts the vector field from records, delegates to the adapter
//! for similarity search. Returns `IndexResult::Ranked`.

use super::adapter::{VectorAdapter, VectorError};
use crate::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::write_ops::IndexWriteOp;
use async_trait::async_trait;
use futures::StreamExt;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::{RecordRef, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
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

    /// Resolve `self.field_path` to its interned-key form.
    fn ipath(&self) -> SmallVec<[InternerKey; 4]> {
        self.field_path
            .iter()
            .map(|&id| InternerKey::new(id))
            .collect()
    }

    /// Extract the embedding vector from `rec` via `any_seq_elem`.
    ///
    /// Parity with the legacy manual `InnerValue::List` walk: returns
    /// `Some(Vec<f32>)` iff the field at `ipath` is a sequence whose
    /// elements are ALL numeric (F64/Int); returns `None` otherwise
    /// (non-sequence leaf, or any non-numeric scalar element).
    ///
    /// **Widening note vs. legacy:** `any_seq_elem` accepts List OR Set
    /// (the legacy code only accepted List). Vector fields are always
    /// List in practice, so this is harmless. Additionally,
    /// `any_seq_elem` silently skips non-scalar elements (nested
    /// Map/List/Set, Dec, Big) rather than reporting them to the
    /// callback; a non-scalar element in a vector field would be
    /// silently omitted rather than causing `None`. This never occurs
    /// with well-formed vector data (lists of numbers) and matches the
    /// plan's documented behaviour.
    fn extract_vec(&self, rec: &dyn RecordRef) -> Option<Vec<f32>> {
        let ipath = self.ipath();
        let mut v: Vec<f32> = Vec::with_capacity(4);
        let mut bad = false;
        let is_seq = rec.any_seq_elem(&ipath, &mut |sr| {
            match sr {
                ScalarRef::F64(f) => v.push(f as f32),
                ScalarRef::Int(n) => v.push(n as f32),
                _ => {
                    bad = true;
                    return true; // short-circuit any_seq_elem
                }
            }
            false // keep going
        });
        if is_seq.is_some() && !bad {
            Some(v)
        } else {
            None
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
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if let Some(v) = self.extract_vec(rec) {
            self.adapter.upsert(rid, &v).await.map_err(ve)?;
        }
        Ok(Vec::new())
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        _old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
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
        _rec: &(dyn RecordRef + Sync + '_),
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
        rec: &(dyn RecordRef + Sync + '_),
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
        old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
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
        rec: &(dyn RecordRef + Sync + '_),
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
    async fn staged_vector(
        &self,
        _rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Option<Vec<f32>> {
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
        // Batch size = 1000: large enough that a single HnswAdapter
        // `upsert_batch` call drives a rayon `parallel_insert` over ~1k
        // vectors (well past the ~`1000 * num_threads` sweet spot the
        // hnsw_rs docstring recommends for parallel_insert efficiency),
        // yet bounded so a single batch's owned-Vec materialisation stays
        // in the low-MB range (1000 × 4-byte × dim). The store stream
        // already paginates at this granularity, so we hand each page
        // straight to the adapter as one batch.
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
                if let Some(v) = self.extract_vec(&rec as &dyn RecordRef) {
                    items.push((rid, v));
                }
            }
            // One batched upsert per store page: HnswAdapter overrides
            // `upsert_batch` → single rayon parallel_insert over the page
            // (was N serial inserts in 64-wide inner chunks). The default
            // `upsert_batch` (BruteForceAdapter etc.) falls back to a
            // per-row loop, so the call is correct for every adapter.
            if !items.is_empty() {
                self.adapter.upsert_batch(&items).await.map_err(ve)?;
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
