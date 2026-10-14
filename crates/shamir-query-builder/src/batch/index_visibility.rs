//! Index-apply visibility for transactional commits (builder-side, v4).

/// Index-apply visibility for a transactional batch commit (query-language
/// v4). Replaces the old `Durability::AsyncIndex` variant / the old
/// `durability: "async_index"` wire string. Only meaningful for
/// `transactional: true` batches; ignored otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexVisibility {
    /// Index posting apply runs inline before ack (default).
    Sync,
    /// Ack after WAL fsync + data apply + MVCC publish; index posting
    /// apply, recovery markers, WAL cleanup, and HNSW promote run on a
    /// background task. Shortens the pre-ACK critical section while
    /// preserving WAL durability and read-your-own-writes on data.
    Async,
}
