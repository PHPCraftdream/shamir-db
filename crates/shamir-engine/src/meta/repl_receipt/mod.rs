//! Replication redo receipt + bookmark v3 codecs (S5c-X PR6b-1).
//!
//! INACTIVE: no production path writes receipts or bookmark v3 in this PR.
//! PR6b-2 wires the receipt into follower apply/resume; PR6b-3 migration.
pub mod bookmark_v3;
pub mod receipt_record;
pub use bookmark_v3::{decode_bookmark_v3, encode_bookmark_v3, load_bookmark_v3, BookmarkV3};
pub use receipt_record::{
    decode_receipt, encode_receipt, receipt_id, ReceiptBinding, ReceiptCounterDelta, ReceiptOp,
    ReceiptPhase, ReceiptTimestamp, ReceiptVersionMapping, ReplicationReceipt, MAX_RECEIPT_EVENTS,
    MAX_RECEIPT_OPS,
};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
