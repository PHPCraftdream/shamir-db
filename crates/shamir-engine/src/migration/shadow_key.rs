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
