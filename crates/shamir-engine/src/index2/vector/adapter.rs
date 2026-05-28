//! Abstract adapter trait for vector search backends.
//!
//! Concrete impls: `BruteForceAdapter` (in-process exact KNN),
//! future `HnswAdapter`, external `QdrantAdapter`.

use async_trait::async_trait;
use shamir_tx::TxId;
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
    async fn upsert(&self, rid: RecordId, vec: &[f32], tx: Option<TxId>)
        -> Result<(), VectorError>;
    async fn delete(&self, rid: RecordId, tx: Option<TxId>) -> Result<(), VectorError>;
    async fn search(
        &self,
        query: &[f32],
        k: u32,
        tx: Option<TxId>,
    ) -> Result<Vec<(RecordId, f32)>, VectorError>;
    fn dim(&self) -> u32;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Promote all vectors staged under `tx_id` into the live structure
    /// (transaction commit, HIGH-6). Default no-op: adapters that don't
    /// maintain a per-tx staging buffer (e.g. `BruteForceAdapter`) have
    /// nothing to commit. `HnswAdapter` overrides this.
    async fn commit_staged(&self, _tx_id: TxId) -> Result<(), VectorError> {
        Ok(())
    }

    /// Drop all vectors staged under `tx_id` without touching the live
    /// structure (transaction abort / rollback, HIGH-6). Default no-op.
    /// `HnswAdapter` overrides this.
    async fn rollback_staged(&self, _tx_id: TxId) {}
}
