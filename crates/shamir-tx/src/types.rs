//! Basic transactional types — IDs, isolation levels, error types.

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
///   concurrent tx after the snapshot was taken, the commit aborts
///   with [`TxConflict`]. Client is expected to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    #[default]
    Snapshot,
    Serializable,
}

/// SSI-only conflict: a key the tx read at `snapshot_version` was
/// overwritten by a concurrent committed tx at `winner_version`
/// before this tx could commit.
#[derive(Debug, Clone, Error)]
#[error("tx conflict on key (snapshot v={snapshot_version}, winner v={winner_version})")]
pub struct TxConflict {
    pub snapshot_version: u64,
    pub winner_version: u64,
    pub key_hint: Option<bytes::Bytes>,
}

/// Top-level error type for the transactional layer. Boundary
/// transcripts cross into [`shamir_storage::error::DbError`] via
/// `From` impl; engine code typically converts on entry.
#[derive(Debug, Error)]
pub enum TxError {
    #[error("tx conflict: {0}")]
    Conflict(#[from] TxConflict),

    #[error("tx aborted: {0}")]
    Aborted(String),

    #[error("tx too large: {ops} ops exceeds limit {limit}")]
    TooLarge { ops: usize, limit: usize },

    #[error("storage error: {0}")]
    Storage(#[from] shamir_storage::error::DbError),

    #[error("cross-repo tx not supported")]
    CrossRepoNotSupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_id_display() {
        assert_eq!(TxId::new(42).to_string(), "tx#42");
    }

    #[test]
    fn isolation_level_serde_roundtrip() {
        let levels = [IsolationLevel::Snapshot, IsolationLevel::Serializable];
        for lvl in &levels {
            let s = serde_json::to_string(lvl).unwrap();
            let back: IsolationLevel = serde_json::from_str(&s).unwrap();
            assert_eq!(*lvl, back);
        }
        // Wire format check
        assert_eq!(
            serde_json::to_string(&IsolationLevel::Snapshot).unwrap(),
            r#""snapshot""#
        );
        assert_eq!(
            serde_json::to_string(&IsolationLevel::Serializable).unwrap(),
            r#""serializable""#
        );
    }

    #[test]
    fn isolation_level_default_is_snapshot() {
        assert_eq!(IsolationLevel::default(), IsolationLevel::Snapshot);
    }

    #[test]
    fn tx_conflict_display_includes_versions() {
        let c = TxConflict {
            snapshot_version: 5,
            winner_version: 10,
            key_hint: None,
        };
        let s = format!("{c}");
        assert!(s.contains("snapshot v=5"));
        assert!(s.contains("winner v=10"));
    }
}
