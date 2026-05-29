//! Async trait every concrete index backend implements.
//!
//! The `IndexManager` (current `index/`) will be rewritten to dispatch
//! through `Arc<dyn IndexBackend>`. Each variant (Btree, Functional,
//! FTS, Vector) lives behind its own `IndexBackend` impl.

use crate::index2::descriptor::IndexDescriptor;
use crate::index2::write_ops::IndexWriteOp;
use async_trait::async_trait;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use std::ops::Bound;
use std::sync::Arc;

#[derive(Debug)]
pub enum IndexQuery {
    /// Equality / `IN` — one or more exact keys.
    Point { keys: SmallVec<[Vec<u8>; 4]> },
    /// Range lookup (`Gt` / `Lt` / `Between`).
    Range {
        lo: Bound<Vec<u8>>,
        hi: Bound<Vec<u8>>,
    },
    /// FTS — interned token IDs + combination mode.
    Fts { tokens: Vec<u64>, mode: FtsMode },
    /// Vector similarity (top-k by `kind`'s metric).
    Vector { vec: Vec<f32>, k: u32 },
}

#[derive(Debug, Clone, Copy)]
pub enum FtsMode {
    AndAll,
    OrAny,
}

#[derive(Debug)]
pub enum IndexResult {
    /// Unordered membership (Btree / Functional / FTS without scoring).
    Set(BTreeSet<RecordId>),
    /// Ranked top-k with score (BM25 / Vector).
    Ranked(Vec<(RecordId, f32)>),
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("backend error: {0}")]
    Backend(String),
}

#[async_trait]
pub trait IndexBackend: Send + Sync {
    fn descriptor(&self) -> &IndexDescriptor;

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError>;

    async fn rebuild(&self, source: Arc<dyn Store>) -> Result<(), IndexError>;
    async fn drop_all(&self) -> Result<(), IndexError>;

    /// tx-aware lookup variant.
    ///
    /// `staged_vectors` is the calling tx's own un-committed vectors for
    /// this table (`TxContext::staged_vectors_for(token)`), resolved by
    /// the caller which knows the table token. The default impl forwards
    /// to [`lookup`] and ignores it; only `VectorBackend` overrides this
    /// to merge the staged vectors into a similarity search so an in-tx
    /// query sees its own writes (HIGH-6). Non-vector backends stage
    /// their postings in `tx.index_write_set` and have nothing to merge
    /// here.
    async fn lookup_tx(
        &self,
        query: IndexQuery,
        _staged_vectors: Option<&[(RecordId, Vec<f32>)]>,
    ) -> Result<IndexResult, IndexError> {
        self.lookup(query).await
    }

    /// Plan ops for an insert.
    async fn plan_insert(
        &self,
        _rid: RecordId,
        _rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        Ok(Vec::new())
    }

    async fn plan_update(
        &self,
        _rid: RecordId,
        _old: &InnerValue,
        _new: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        Ok(Vec::new())
    }

    async fn plan_delete(
        &self,
        _rid: RecordId,
        _rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        Ok(Vec::new())
    }

    /// tx-aware planning variant of [`plan_insert`].
    ///
    /// Default forwards to `plan_insert` (tx-unaware). Backends that
    /// maintain non-storage side state — e.g. `VectorBackend` with its
    /// HNSW graph — override this so a `Some(tx_id)` call does NOT touch
    /// the live structure: the vector itself is staged into the tx via
    /// [`staged_vector`] / `TxContext::stage_vector` and promoted at
    /// commit. That way a dropped (rolled-back) tx leaves no ghost state
    /// on the live structure. See HIGH-6.
    async fn plan_insert_tx(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        _tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        self.plan_insert(rid, rec).await
    }

    /// tx-aware planning variant of [`plan_update`]. See [`plan_insert_tx`].
    async fn plan_update_tx(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
        _tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        self.plan_update(rid, old, new).await
    }

    /// tx-aware planning variant of [`plan_delete`]. See [`plan_insert_tx`].
    async fn plan_delete_tx(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        _tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        self.plan_delete(rid, rec).await
    }

    /// Apply in-memory-only ops (e.g. BumpFtsStats). Called by
    /// `apply_index_ops` for ops that don't go through the Store.
    /// Default: no-op.
    async fn apply_in_memory(&self, _ops: &[IndexWriteOp]) -> Result<(), IndexError> {
        Ok(())
    }

    /// Extract the vector this backend would stage for `rec`, if any.
    ///
    /// The executor calls this on the tx-aware write path and routes the
    /// returned vector into `TxContext::staged_vectors` (HIGH-6). Default
    /// `None` — only `VectorBackend` extracts its embedding field; every
    /// other backend stages its state as `IndexWriteOp`s instead.
    async fn staged_vector(&self, _rid: RecordId, _rec: &InnerValue) -> Option<Vec<f32>> {
        None
    }

    /// Promote the tx's staged vectors for this table into the live
    /// structure at commit (commit pipeline Phase 5d, HIGH-6). `vecs` is
    /// the tx's `staged_vectors` slice for the owning table. Default
    /// no-op — only `VectorBackend` overrides it to feed the HNSW graph.
    /// Abort needs no counterpart: a dropped tx discards `staged_vectors`
    /// by RAII, so the live structure is never touched until commit.
    async fn apply_staged_vectors(&self, _vecs: &[(RecordId, Vec<f32>)]) -> Result<(), IndexError> {
        Ok(())
    }
}
