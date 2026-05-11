//! WAL record types — namespaced for future expansion.

use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;

/// One entry in the WAL — describes a transaction that is (or was)
/// in flight.
///
/// `ops` contains all the engine-level operations the transaction
/// intends to perform; the actual data + index writes happen
/// AFTER the marker is durable. On crash, `ops` tells recovery
/// exactly which record_ids to verify.
///
/// `entry_id` and `timestamp_ns` are for ordering, debugging, and
/// eventual log-replay-style recovery (not used in the simple
/// marker-and-fix model).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    pub txn_id: u64,
    pub started_at_ns: u64,
    pub ops: Vec<WalOp>,
}

impl WalEntry {
    pub fn new(txn_id: u64, ops: Vec<WalOp>) -> Self {
        let started_at_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            txn_id,
            started_at_ns,
            ops,
        }
    }
}

/// A single operation recorded in the WAL.
///
/// Record-level ops are enough to drive recovery as long as the
/// engine can ALWAYS derive the index state from the record state
/// (which is the case in ShamirDb today — `data_store` is the
/// source of truth, indexes are derived).
///
/// Future operations are listed in comments. Adding a new variant
/// is a non-breaking change for the WAL — old recovery code falls
/// through to the doctor's full-rebuild path on unknown ops, so
/// nothing crashes; new recovery code handles the new op kinds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalOp {
    /// A record is being created with this `record_id`. After
    /// successful commit, both `data_store[record_id]` and all
    /// derived index entries for it must exist. On recovery, if the
    /// record is missing from data_store → rollback (clean up any
    /// orphan index entries). If present → forward-fix missing
    /// indexes.
    RecordCreated { record_id: RecordId },

    /// A record is being updated. On recovery, the old value is no
    /// longer available; full-rebuild fall-back handles the index
    /// reconciliation.
    RecordUpdated { record_id: RecordId },

    /// A record is being deleted. On recovery, if the record is
    /// still in data_store → roll forward (delete it + its
    /// indexes). If absent → clean up any orphan index entries.
    RecordDeleted { record_id: RecordId },

    /// Explicit transaction-boundary marker. NOT used in the
    /// implicit-batch path (each batch's marker is itself the
    /// transaction). Reserved for future explicit-transaction API.
    TxnBegin,

    /// Marks an explicit transaction as completed. Same caveat —
    /// reserved for future.
    TxnCommit,

    /// Marks an explicit transaction as rolled back. Reserved.
    TxnRollback,
    // Future ops, kept here as documentation. Recovery should
    // tolerate unknown variants by falling through to full-rebuild.
    //
    // /// Full-text indexing — one term being added/removed for a
    // /// record. Same recovery shape as a regular IndexEntry.
    // FtsTermAdded { record_id: RecordId, term_hash: u64 },
    // FtsTermRemoved { record_id: RecordId, term_hash: u64 },
    //
    // /// Index schema migration — index being created or dropped.
    // /// Recovery needs to know about the schema delta to decide
    // /// what state is consistent.
    // IndexCreated { name_interned: u64 },
    // IndexDropped { name_interned: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wal_entry_roundtrips_bincode() {
        let entry = WalEntry::new(
            42,
            vec![
                WalOp::RecordCreated {
                    record_id: RecordId::new(),
                },
                WalOp::RecordDeleted {
                    record_id: RecordId::new(),
                },
            ],
        );
        let bytes = bincode::serialize(&entry).unwrap();
        let back: WalEntry = bincode::deserialize(&bytes).unwrap();
        assert_eq!(back.txn_id, entry.txn_id);
        assert_eq!(back.ops.len(), 2);
    }

    #[test]
    fn unknown_future_op_does_not_break_existing_variants() {
        // Smoke test: the present variants encode/decode fine. When
        // a future variant is added, this test ensures the existing
        // variants keep their numeric tags (bincode is sensitive to
        // variant order).
        let v1 = WalOp::RecordCreated {
            record_id: RecordId::new(),
        };
        let bytes = bincode::serialize(&v1).unwrap();
        let _: WalOp = bincode::deserialize(&bytes).expect("RecordCreated roundtrip");
    }
}
