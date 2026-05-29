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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    #[default]
    Snapshot,
    Serializable,
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
}
