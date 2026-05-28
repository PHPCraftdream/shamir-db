//! Async trait every concrete index backend implements.
//!
//! The `IndexManager` (current `index/`) will be rewritten to dispatch
//! through `Arc<dyn IndexBackend>`. Each variant (Btree, Functional,
//! FTS, Vector) lives behind its own `IndexBackend` impl.

use crate::index2::descriptor::IndexDescriptor;
use crate::index2::write_ops::IndexWriteOp;
use async_trait::async_trait;
use shamir_storage::types::Store;
use shamir_tx::TxContext;
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
    /// In the current sub-stage (3.2.C) the default implementation
    /// forwards to [`lookup`] and ignores the `tx` parameter. Backends
    /// that want to merge committed postings with in-tx staged index
    /// writes will override this method in later sub-stages (see
    /// docs/pre-transactional/04-mvcc-store.md §3.2 / §3.3).
    async fn lookup_tx(
        &self,
        query: IndexQuery,
        _tx: Option<&TxContext>,
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
    /// HNSW graph — override this to route per-tx mutations into a
    /// per-tx staging area instead of the live structure. That way a
    /// dropped (rolled-back) tx leaves no ghost postings on the live
    /// state. See HIGH-6.
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

    /// Promote any per-tx staged side state for `tx_id` into the live
    /// structure at transaction commit (commit pipeline Phase 5d,
    /// HIGH-6). Default no-op — only backends that maintain non-storage
    /// staging (e.g. `VectorBackend`'s HNSW graph) override this.
    async fn commit_staged_tx(&self, _tx_id: shamir_tx::TxId) -> Result<(), IndexError> {
        Ok(())
    }

    /// Drop any per-tx staged side state for `tx_id` on transaction
    /// abort / rollback (HIGH-6). Default no-op. See [`commit_staged_tx`].
    async fn rollback_staged_tx(&self, _tx_id: shamir_tx::TxId) {}
}
