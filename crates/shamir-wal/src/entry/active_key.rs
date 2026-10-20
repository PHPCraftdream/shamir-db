//! Typed encoding for WAL active marker keys.
//!
//! Physical layout: `b"__wal_active_" || txn_id_be_u64` = 21 bytes.
//! The byte literal stays as a single private const here so the
//! encoding lives in one place instead of being recomputed at three
//! callsites.
//!
//! BE encoding for the txn_id suffix means lexicographic byte-order
//! over active keys matches numeric order over txn_id — useful for
//! recovery's `scan_prefix → sorted by oldest first` flow.

use bytes::{BufMut, Bytes, BytesMut};
use shamir_storage::types::RecordKey;

const PREFIX: &[u8] = b"__wal_active_";
const KEY_LEN: usize = PREFIX.len() + 8;

/// Typed wrapper over the `__wal_active_<txn_id>` physical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalActiveKey {
    pub txn_id: u64,
}

impl WalActiveKey {
    pub fn new(txn_id: u64) -> Self {
        Self { txn_id }
    }

    /// Encode to the canonical physical layout used by `WalManager`.
    pub fn to_bytes(self) -> Bytes {
        let mut b = BytesMut::with_capacity(KEY_LEN);
        b.extend_from_slice(PREFIX);
        b.put_u64(self.txn_id);
        b.freeze()
    }

    pub fn to_record_key(self) -> RecordKey {
        RecordKey::from(self.to_bytes())
    }

    /// Scan prefix for `list_inflight` (no txn_id appended).
    pub fn scan_prefix() -> Bytes {
        Bytes::copy_from_slice(PREFIX)
    }

    /// Parse a physical key back into a `txn_id`. Returns `None` for
    /// keys that don't match the expected layout (wrong length /
    /// missing prefix).
    pub fn parse(key: &[u8]) -> Option<u64> {
        if key.len() != KEY_LEN || !key.starts_with(PREFIX) {
            return None;
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&key[PREFIX.len()..]);
        Some(u64::from_be_bytes(bytes))
    }
}
