//! Typed encoding for migration shadow-log keys.
//!
//! Physical layout (variable length):
//!   `b"__shadow_" || id_len_be_u32 || migration_id_bytes || lsn_be_u64`
//!
//! The `migration_id` is LENGTH-PREFIXED rather than delimiter-separated.
//! Production ids are `mig_<table_name>_<ts>_<hex>`
//! (`shamir-db`'s `admin_migration.rs`), where `<table_name>` is a
//! user-controlled identifier that may itself contain `_` — so the id
//! already routinely contains interior `_` bytes. A delimiter scheme
//! (the prior `id || b'_' || lsn` layout) is ambiguous for such ids:
//! `scan_prefix("A")` is a byte-prefix of every key belonging to a
//! DIFFERENT migration `"A_B"` (`"__shadow_A_"` literally prefixes
//! `"__shadow_A_B_..."`), so one migration's shadow log could bleed
//! into another's at the prefix-scan boundary.
//!
//! Length-prefixing removes the ambiguity for ANY id content: two
//! distinct ids either encode a different length (their key streams
//! diverge inside the length field, before the id bytes even start) or
//! the same length (in which case matching id bytes implies identical
//! strings) — no id can ever be a byte-prefix of another id's keyspace.

use bytes::Bytes;
use shamir_storage::types::RecordKey;

const PREFIX: &[u8] = b"__shadow_";
const ID_LEN_BYTES: usize = 4;
const LSN_BYTES: usize = 8;

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
        let id_bytes = self.migration_id.as_bytes();
        let mut k = Vec::with_capacity(PREFIX.len() + ID_LEN_BYTES + id_bytes.len() + LSN_BYTES);
        k.extend_from_slice(PREFIX);
        k.extend_from_slice(&(id_bytes.len() as u32).to_be_bytes());
        k.extend_from_slice(id_bytes);
        k.extend_from_slice(&self.lsn.to_be_bytes());
        Bytes::from(k)
    }

    pub fn to_record_key(&self) -> RecordKey {
        RecordKey::from(self.to_bytes())
    }

    /// Scan prefix for a given migration (no lsn appended). Unambiguous
    /// for any id content — see module doc.
    pub fn scan_prefix(migration_id: &str) -> Bytes {
        let id_bytes = migration_id.as_bytes();
        let mut k = Vec::with_capacity(PREFIX.len() + ID_LEN_BYTES + id_bytes.len());
        k.extend_from_slice(PREFIX);
        k.extend_from_slice(&(id_bytes.len() as u32).to_be_bytes());
        k.extend_from_slice(id_bytes);
        Bytes::from(k)
    }

    /// Extract the LSN suffix from a physical key. Returns `None`
    /// if the key is shorter than 8 bytes — does NOT validate the
    /// prefix shape (caller already filtered by `scan_prefix`).
    pub fn parse_lsn(key: &[u8]) -> Option<u64> {
        if key.len() < LSN_BYTES {
            return None;
        }
        let tail = &key[key.len() - LSN_BYTES..];
        Some(u64::from_be_bytes(tail.try_into().ok()?))
    }
}
