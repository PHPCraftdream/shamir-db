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
/// `counter_delta` is the net change to the record counter that
/// this transaction was about to apply. Targeted recovery uses
/// it to reconcile the counter in O(1) without a full data-store
/// scan: e.g. an `insert_many` of N records sets `counter_delta =
/// +N`; a `delete_many` of K records sets `counter_delta = -K`;
/// an UPDATE that doesn't change row count sets `counter_delta =
/// 0`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    pub txn_id: u64,
    pub started_at_ns: u64,
    pub counter_delta: i64,
    pub ops: Vec<WalOp>,
}

impl WalEntry {
    pub fn new(txn_id: u64, ops: Vec<WalOp>) -> Self {
        Self::new_with_delta(txn_id, ops, 0)
    }

    pub fn new_with_delta(txn_id: u64, ops: Vec<WalOp>, counter_delta: i64) -> Self {
        let started_at_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            txn_id,
            started_at_ns,
            counter_delta,
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
