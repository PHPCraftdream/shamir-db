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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for txn_id in [0u64, 1, 42, u64::MAX / 2, u64::MAX] {
            let k = WalActiveKey::new(txn_id);
            let bytes = k.to_bytes();
            assert_eq!(WalActiveKey::parse(&bytes), Some(txn_id));
        }
    }

    #[test]
    fn binary_layout_matches_legacy() {
        // Byte-for-byte compatibility with the previous inline
        // `ACTIVE_PREFIX || txn_id_be` encoding. Persisted WAL data
        // on disk before this refactor MUST still parse correctly.
        let k = WalActiveKey::new(0x1234_5678_9abc_def0).to_bytes();
        let expected: &[u8] = &[
            b'_', b'_', b'w', b'a', b'l', b'_', b'a', b'c', b't', b'i', b'v', b'e', b'_', 0x12,
            0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
        ];
        assert_eq!(k.as_ref(), expected, "binary layout must not change");
        assert_eq!(k.len(), 21);
    }

    #[test]
    fn parse_rejects_wrong_length() {
        assert_eq!(WalActiveKey::parse(b""), None);
        assert_eq!(WalActiveKey::parse(b"__wal_active_"), None);
        assert_eq!(WalActiveKey::parse(&[0u8; 22]), None);
    }

    #[test]
    fn parse_rejects_wrong_prefix() {
        let bad: [u8; 21] = [
            b'_', b'_', b'd', b'a', b'l', b'_', b'a', b'c', b't', b'i', b'v', b'e', b'_', 0, 0, 0,
            0, 0, 0, 0, 0,
        ];
        assert_eq!(WalActiveKey::parse(&bad), None);
    }

    #[test]
    fn scan_prefix_matches_legacy() {
        let prefix = WalActiveKey::scan_prefix();
        assert_eq!(prefix.as_ref(), b"__wal_active_");
    }
}
