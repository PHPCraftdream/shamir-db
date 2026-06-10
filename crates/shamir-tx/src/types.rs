//! Basic transactional types — IDs and isolation levels.

use serde::{Deserialize, Serialize};

/// Monotonic per-repo transaction identifier.
///
/// Allocated by `RepoTxGate::fresh_txn_id` at the start of every tx.
/// Recovery on open reads `SysKey::LastTxId` to seed the counter so
/// IDs never collide with already-committed transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TxId(pub u64);

impl TxId {
    pub const fn new(v: u64) -> Self {
        Self(v)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tx#{}", self.0)
    }
}

/// Isolation level requested by a transactional batch.
///
/// - [`Snapshot`] — reads see a consistent snapshot taken at tx start;
///   commits last-writer-wins (no read-set validation). Default.
/// - [`Serializable`] — same as snapshot, plus read-set validation
///   at commit: if any key the tx read has been overwritten by a
///   concurrent tx after the snapshot was taken, the commit aborts.
///   Client is expected to retry. (The engine-side commit pipeline
///   surfaces this via `shamir_engine::tx::commit::TxError::SsiConflict`.)
/// - [`Pessimistic`] — Level-3: per-key locks with wound-wait. The tx's
///   monotonic id is its priority (smaller id = older = higher priority).
///   Reads acquire `Shared` locks, writes acquire `Exclusive` locks, both
///   through [`MvccStore::lock_key`](crate::mvcc_store::MvccStore::lock_key).
///   Deadlock-free by construction: a tx only ever waits on strictly-older
///   holders and only ever wounds strictly-younger ones, so the wait-for
///   graph respects the total id order and cannot cycle. Snapshot and
///   Serializable behavior are byte-identical when no `Pessimistic` tx runs
///   (the locks registry stays empty → zero overhead on the hot paths).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    #[default]
    Snapshot,
    Serializable,
    Pessimistic,
}
