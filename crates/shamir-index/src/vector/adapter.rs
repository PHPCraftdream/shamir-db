//! Abstract adapter trait for vector search backends.
//!
//! Concrete impls: `BruteForceAdapter` (in-process exact KNN),
//! future `HnswAdapter`, external `QdrantAdapter`.

use async_trait::async_trait;
use shamir_types::types::record_id::RecordId;

#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimMismatch { expected: u32, got: u32 },
    #[error("adapter error: {0}")]
    Internal(String),
}

#[async_trait]
pub trait VectorAdapter: Send + Sync {
    /// Insert (or replace) `rid`'s vector in the live structure
    /// immediately. Transactional staging does NOT go through here — the
    /// executor buffers per-tx vectors in `TxContext::staged_vectors` and
    /// promotes them at commit via [`apply_committed_vectors`].
    async fn upsert(&self, rid: RecordId, vec: &[f32]) -> Result<(), VectorError>;
    async fn delete(&self, rid: RecordId) -> Result<(), VectorError>;

    /// Top-k search over the committed structure, optionally merged with
    /// the caller's own un-committed staged vectors.
    ///
    /// `staged` is the slice from `TxContext::staged_vectors_for(token)`
    /// (resolved by the caller, which knows the table token). `None` for a
    /// plain non-tx search. When present, the staged vectors are scored
    /// brute-force and merged into the result so an in-tx query sees its
    /// own writes.
    async fn search(
        &self,
        query: &[f32],
        k: u32,
        staged: Option<&[(RecordId, Vec<f32>)]>,
    ) -> Result<Vec<(RecordId, f32)>, VectorError>;

    fn dim(&self) -> u32;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Promote a batch of committed vectors into the live structure at
    /// transaction commit (commit pipeline Phase 5d, HIGH-6). Called with
    /// the tx's own `staged_vectors` for this table; equivalent to a
    /// non-tx `upsert` per pair, so the default impl covers every adapter.
    async fn apply_committed_vectors(
        &self,
        vecs: &[(RecordId, Vec<f32>)],
    ) -> Result<(), VectorError> {
        for (rid, vec) in vecs {
            self.upsert(*rid, vec).await?;
        }
        Ok(())
    }
}
