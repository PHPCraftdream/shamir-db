//! Abstract adapter trait for vector search backends.
//!
//! Concrete impls: `BruteForceAdapter` (in-process exact KNN),
//! `HnswAdapter` (`hnsw_rs`), future external `QdrantAdapter`.

use async_trait::async_trait;
use shamir_types::types::record_id::RecordId;

#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimMismatch { expected: u32, got: u32 },
    #[error("adapter error: {0}")]
    Internal(String),
}

/// Per-query tuning knobs for approximate vector search (HNSW).
///
/// Both fields are `Option` so that `None` (the default) means "use the
/// adapter's build-time default" — a request that omits the struct entirely
/// behaves identically to pre-V1.1 code, which is what back-compat requires.
///
/// * `ef_search` — the HNSW `ef` exploration-width at query time. Higher =
///   better recall, higher latency. When `Some`, it overrides the adapter's
///   `HnswConfig::ef_search`; clamped to `MAX_EF_SEARCH` before use.
/// * `oversample` — reserved for P3 (#404): a multiplier that widens the
///   candidate set before reranking/truncation. Accepted on the wire and
///   threaded through `IndexQuery::Vector` so the `search` signature is stable,
///   but NO backend consumes it yet. Stored here so a single struct covers the
///   whole per-query surface.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SearchOpts {
    pub ef_search: Option<u32>,
    pub oversample: Option<f32>,
}

impl SearchOpts {
    /// Convenience for the common "just ef_search" case.
    pub fn with_ef_search(ef_search: u32) -> Self {
        Self {
            ef_search: Some(ef_search),
            oversample: None,
        }
    }

    /// `true` when both knobs are unset — callers can skip any per-query
    /// bookkeeping and use the adapter default directly.
    pub fn is_default(&self) -> bool {
        self.ef_search.is_none() && self.oversample.is_none()
    }
}

impl From<u32> for SearchOpts {
    fn from(ef_search: u32) -> Self {
        Self::with_ef_search(ef_search)
    }
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
    ///
    /// `opts` carries per-query tuning knobs (`ef_search`, `oversample`).
    /// `Default` (`None` on both) preserves the pre-V1.1 behaviour (the
    /// adapter's build-time default). BruteForce ignores `ef_search` (exact
    /// search has no width knob) and `oversample` is not yet wired (P3 #404).
    async fn search(
        &self,
        query: &[f32],
        k: u32,
        opts: SearchOpts,
        staged: Option<&[(RecordId, Vec<f32>)]>,
    ) -> Result<Vec<(RecordId, f32)>, VectorError>;

    fn dim(&self) -> u32;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Batch upsert: insert or replace many `(rid, vec)` pairs at once.
    ///
    /// Default impl naively loops `upsert`; concrete adapters may override
    /// (e.g. `HnswAdapter` does a single rayon `parallel_insert` over the
    /// whole batch — amortizing the graph-insert overhead). The contract:
    /// EITHER every row is applied OR none are (atomic on dim-mismatch —
    /// all dims are validated up front before the first insert).
    async fn upsert_batch(&self, items: &[(RecordId, Vec<f32>)]) -> Result<(), VectorError> {
        for (rid, vec) in items {
            self.upsert(*rid, vec).await?;
        }
        Ok(())
    }

    /// Promote a batch of committed vectors into the live structure at
    /// transaction commit (commit pipeline Phase 5d, HIGH-6). Called with
    /// the tx's own `staged_vectors` for this table; equivalent to a
    /// non-tx `upsert` per pair, so the default impl covers every adapter.
    async fn apply_committed_vectors(
        &self,
        vecs: &[(RecordId, Vec<f32>)],
    ) -> Result<(), VectorError> {
        // Prefer a batched path when the adapter overrides `upsert_batch`
        // (HnswAdapter does one rayon `parallel_insert` instead of N
        // serial inserts). The default `upsert_batch` falls back to the
        // same per-row loop, so this is strictly ≥ the old behaviour.
        self.upsert_batch(vecs).await
    }
}
