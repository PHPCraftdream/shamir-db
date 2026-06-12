/// Per-request durability level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Ack after in-memory buffer; durable on background tick.
    Buffered,
    /// Flush durable backing before ack.
    Synced,
    /// Ack after WAL fsync + data apply + MVCC publish; index posting apply,
    /// recovery markers, WAL marker removal, and HNSW promote run on a
    /// background task. Shortens the pre-ACK critical section while preserving
    /// WAL durability and read-your-own-writes on data. Only meaningful for
    /// `transactional: true` batches.
    AsyncIndex,
}

impl Durability {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Durability::Buffered => "buffered",
            Durability::Synced => "synced",
            Durability::AsyncIndex => "async_index",
        }
    }
}
