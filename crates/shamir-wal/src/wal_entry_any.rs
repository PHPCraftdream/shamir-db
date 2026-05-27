//! Read-side enum that distinguishes V1 (non-tx) from V2 (tx) WAL
//! entries. Both share the same `WalActiveKey` prefix; the reader
//! sniffs the V2 magic byte sequence to dispatch.

use crate::wal_entry::WalEntry;
use crate::wal_entry_v2::WalEntryV2;

/// Either a V1 (non-transactional, `bincode(WalEntry)`) or V2
/// (transactional, `WAL2 || version || bincode(WalEntryV2)`) entry
/// recovered from the active-marker prefix.
#[derive(Debug, Clone)]
pub enum WalEntryAny {
    V1(WalEntry),
    V2(WalEntryV2),
}

impl WalEntryAny {
    /// The transaction id embedded in either variant.
    pub fn txn_id(&self) -> u64 {
        match self {
            WalEntryAny::V1(e) => e.txn_id,
            WalEntryAny::V2(e) => e.txn_id,
        }
    }
}
