//! Pure-data index write operation enum.
//!
//! Moved here from `shamir-engine::index2::write_ops` so that
//! `TxContext` (which lives in `shamir-tx`) can reference it without
//! a circular dependency. `shamir-engine` re-exports it via `pub use`.

use bytes::Bytes;

/// A single planned index mutation. Returned by `IndexBackend::plan_*`
/// methods; applied either immediately (non-tx) or accumulated in
/// `TxContext.index_write_set` for atomic commit (tx).
#[derive(Debug, Clone)]
pub enum IndexWriteOp {
    /// Insert or overwrite a posting in the index store.
    SetPosting { key: Bytes, value: Bytes },
    /// Delete a posting by key from the index store.
    RemovePosting { key: Bytes },
    /// Bump FtsRankedBackend's in-memory BM25 stats (doc_count +
    /// sum_doc_len). `sign` is +1 for insert, -1 for delete.
    /// Never persisted as a posting — applied via `apply_in_memory`.
    BumpFtsStats { doc_len: u32, sign: i8 },
}
