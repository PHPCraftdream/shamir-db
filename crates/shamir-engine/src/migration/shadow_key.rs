//! Typed encoding for migration shadow-log keys.
//!
//! Physical layout (variable length):
//!   `b"__shadow_" || migration_id || b'_' || lsn_be_u64`
//!
//! The `migration_id` is a UTF-8 string; it must not contain
//! interior `b'_'` ambiguity that would break parse — currently
//! migration_ids are UUIDs or short ASCII identifiers, so safe.

use bytes::Bytes;
use shamir_storage::types::RecordKey;

const PREFIX: &[u8] = b"__shadow_";

/// Typed wrapper for shadow-log entry keys.
#[derive(Debug, Clone)]
pub struct ShadowKey<'a> {
    pub migration_id: &'a str,
    pub lsn: u64,
}

impl<'a> ShadowKey<'a> {
    pub fn new(migration_id: &'a str, lsn: u64) -> Self {
        Self { migration_id, lsn }
    }

    /// Physical key for this entry.
    pub fn to_bytes(&self) -> Bytes {
        let mut k = Vec::with_capacity(PREFIX.len() + self.migration_id.len() + 1 + 8);
        k.extend_from_slice(PREFIX);
        k.extend_from_slice(self.migration_id.as_bytes());
        k.push(b'_');
        k.extend_from_slice(&self.lsn.to_be_bytes());
        Bytes::from(k)
    }

    pub fn to_record_key(&self) -> RecordKey {
        RecordKey::from(self.to_bytes())
    }

    /// Scan prefix for a given migration (no lsn appended).
    pub fn scan_prefix(migration_id: &str) -> Bytes {
        let mut k = Vec::with_capacity(PREFIX.len() + migration_id.len() + 1);
        k.extend_from_slice(PREFIX);
        k.extend_from_slice(migration_id.as_bytes());
        k.push(b'_');
        Bytes::from(k)
    }

    /// Extract the LSN suffix from a physical key. Returns `None`
    /// if the key is shorter than 8 bytes — does NOT validate the
    /// prefix shape (caller already filtered by `scan_prefix`).
    pub fn parse_lsn(key: &[u8]) -> Option<u64> {
        if key.len() < 8 {
            return None;
        }
        let tail = &key[key.len() - 8..];
        Some(u64::from_be_bytes(tail.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let k = ShadowKey::new("mig-001", 42);
        let bytes = k.to_bytes();
        assert_eq!(ShadowKey::parse_lsn(&bytes), Some(42));
    }

    #[test]
    fn binary_layout_matches_legacy() {
        let bytes = ShadowKey::new("mig-001", 1).to_bytes();
        let mut expected = Vec::new();
        expected.extend_from_slice(b"__shadow_");
        expected.extend_from_slice(b"mig-001");
        expected.push(b'_');
        expected.extend_from_slice(&1u64.to_be_bytes());
        assert_eq!(bytes.as_ref(), expected.as_slice());
    }

    #[test]
    fn scan_prefix_matches_legacy() {
        let prefix = ShadowKey::scan_prefix("mig-001");
        let mut expected = Vec::new();
        expected.extend_from_slice(b"__shadow_");
        expected.extend_from_slice(b"mig-001");
        expected.push(b'_');
        assert_eq!(prefix.as_ref(), expected.as_slice());
    }

    #[test]
    fn parse_lsn_extracts_be_suffix() {
        let k = ShadowKey::new("x", 0xdead_beef_cafe_babe).to_bytes();
        assert_eq!(ShadowKey::parse_lsn(&k), Some(0xdead_beef_cafe_babe));
    }
}
