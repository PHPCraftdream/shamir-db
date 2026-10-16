//! Replication redo receipt + bookmark v3 codecs (S5c-X PR6b-1/PR6b-3).
//!
//! Bookmark records are strict v3-only: anything else is refused (see
//! `bookmark_v3` and `bookmark`).
pub mod bookmark;
pub mod bookmark_v3;
pub mod fence_record;
pub mod receipt;
pub use bookmark_v3::{
    classify_follower_bookmark, decode_bookmark_v3, encode_bookmark_v3, load_bookmark_v3,
    save_bookmark_v3, BookmarkV3, FollowerBookmarkState,
};
pub use receipt::{
    decode_receipt, encode_receipt, receipt_id, ReceiptBinding, ReceiptCounterDelta, ReceiptOp,
    ReceiptPhase, ReceiptTimestamp, ReceiptVersionMapping, ReplicationReceipt, MAX_RECEIPT_EVENTS,
    MAX_RECEIPT_OPS,
};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
