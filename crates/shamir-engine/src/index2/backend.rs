//! Async trait every concrete index backend implements.
//!
//! The `IndexManager` (current `index/`) will be rewritten to dispatch
//! through `Arc<dyn IndexBackend>`. Each variant (Btree, Functional,
//! FTS, Vector) lives behind its own `IndexBackend` impl.

use crate::index2::descriptor::IndexDescriptor;
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

    async fn on_insert(&self, rid: RecordId, rec: &InnerValue) -> Result<(), IndexError>;
    async fn on_update(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> Result<(), IndexError>;
    async fn on_delete(&self, rid: RecordId, rec: &InnerValue) -> Result<(), IndexError>;
    async fn on_batch_insert(&self, items: &[(RecordId, &InnerValue)]) -> Result<(), IndexError>;

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError>;

    async fn rebuild(&self, source: Arc<dyn Store>) -> Result<(), IndexError>;
    async fn drop_all(&self) -> Result<(), IndexError>;
}
