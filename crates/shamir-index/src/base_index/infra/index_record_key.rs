//! Index record key for the base (regular / unique) B-Tree index family —
//! identity v2 layout (design P34-12 sections (a) and (e) S2).
//!
//! The key is:
//! - namespace byte: `0x02` (regular v2) or `0x03` (unique v2) — from
//!   [`crate::model::identity::markers`]; the legacy `0x00`/`0x01`
//!   namespaces are only ever read by the drop/rebuild sweeps
//! - `name_interned` (u64 LE)
//! - identity bytes `I`: the inline canonical bytes `C` (≤256 B) or the
//!   33-byte `0xFF || BLAKE3-256(C)` digest form
//!
//! Total: `9 + |I|` bytes, variable length. The v1 layout
//! (`[ns][name][hash1][hash2]`, fixed 25 bytes, two FxHasher streams) is
//! GONE: this is a breaking persisted-format change with an automatic
//! forward migration (design section (c)); v1 keys are available only via
//! the explicit legacy compatibility constructor below.

use bytes::Bytes;
use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};

use crate::model::identity::markers::{REGULAR_V2, UNIQUE_V2};

/// Index record key for B-Tree (identity v2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecordKey {
    /// Key namespace: [`REGULAR_V2`] (0x02) or [`UNIQUE_V2`] (0x03).
    pub namespace: u8,

    /// Interned ID of the index name (uniquely identifies the index and its fields)
    pub name_interned: u64,

    /// Identity bytes `I` — inline canonical bytes or the digest form.
    pub identity: Vec<u8>,
}

impl IndexRecordKey {
    /// Create a new index record key (identity v2 namespace).
    pub fn new(is_unique: bool, name_interned: u64) -> Self {
        Self {
            namespace: if is_unique { UNIQUE_V2 } else { REGULAR_V2 },
            name_interned,
            identity: Vec::new(),
        }
    }

    /// Legacy v1 constructor used only by migration compatibility tests.
    pub fn legacy_v1(is_unique: bool, name_interned: u64) -> Self {
        Self {
            namespace: if is_unique { 0x01 } else { 0x00 },
            name_interned,
            identity: Vec::new(),
        }
    }

    /// Set the identity bytes `I` (inline `C` or the digest form).
    pub fn with_identity(mut self, identity: Vec<u8>) -> Self {
        self.identity = identity;
        self
    }

    #[deprecated(
        note = "legacy posting compatibility only; new postings use the IDV2 canonical codec"
    )]
    pub fn with_values<T: Hash>(mut self, values: &[&T]) -> Self {
        const SEED2: u64 = 0x9E3779B97F4A7C15;
        let mut h1 = FxHasher::default();
        let mut h2 = FxHasher::default();
        SEED2.hash(&mut h2);
        self.name_interned.hash(&mut h2);
        self.name_interned.hash(&mut h1);
        for value in values {
            value.hash(&mut h1);
            value.hash(&mut h2);
        }
        self.identity.clear();
        self.identity.extend_from_slice(&h1.finish().to_le_bytes());
        self.identity.extend_from_slice(&h2.finish().to_le_bytes());
        self
    }

    /// Deprecated compatibility setter for callers constructing a legacy
    /// v1 key with `legacy_v1`; production paths use `with_identity`.
    #[deprecated(
        note = "legacy posting compatibility only; new postings use the IDV2 canonical codec"
    )]
    pub fn with_hash(mut self, hash1: u64, hash2: u64) -> Self {
        self.identity.clear();
        self.identity.extend_from_slice(&hash1.to_le_bytes());
        self.identity.extend_from_slice(&hash2.to_le_bytes());
        self
    }

    /// Convert to bytes for storage: `[ns][name LE][I]`.
    pub fn to_bytes(&self) -> Bytes {
        let mut bytes = Vec::with_capacity(9 + self.identity.len());
        bytes.push(self.namespace);
        bytes.extend_from_slice(&self.name_interned.to_le_bytes());
        bytes.extend_from_slice(&self.identity);
        Bytes::from(bytes)
    }

    /// Convert to prefix bytes (namespace + name, without the identity)
    /// for scanning. Returns BOTH the v2 and the legacy-v1 prefix of this
    /// index: the migration sweeps (design section (c)) must cover BOTH
    /// namespaces, and legacy postings coexist with v2 postings until the
    /// migration has removed them.
    pub fn to_prefix_bytes(&self) -> Bytes {
        let mut bytes = [0u8; 9];
        bytes[0] = self.namespace;
        bytes[1..9].copy_from_slice(&self.name_interned.to_le_bytes());
        Bytes::copy_from_slice(&bytes)
    }

    /// The legacy-v1 prefix of the same index (`0x00` regular / `0x01`
    /// unique). Used ONLY by the drop/rebuild sweeps so no v1 posting of a
    /// dropped or migrated index survives as garbage.
    pub fn to_legacy_prefix_bytes(&self) -> Bytes {
        let mut bytes = [0u8; 9];
        bytes[0] = match self.namespace {
            UNIQUE_V2 => 0x01,
            _ => 0x00,
        };
        bytes[1..9].copy_from_slice(&self.name_interned.to_le_bytes());
        Bytes::copy_from_slice(&bytes)
    }

    /// Create from bytes (identity v2 layout). Requires at least the 9-byte
    /// prefix; the identity may be empty only for a probe constructed before
    /// `with_identity` — a parsed key with an empty identity is structurally
    /// impossible (every leaf tag is ≥ 1 byte of payload), so it is accepted
    /// here only for round-trip tolerance of a probe key.
    pub fn from_bytes(bytes: Bytes) -> Result<Self, String> {
        if bytes.len() < 10 {
            return Err("IndexRecordKey too short".to_string());
        }

        let namespace = bytes[0];
        let name_interned = u64::from_le_bytes(bytes[1..9].try_into().unwrap());

        Ok(Self {
            namespace,
            name_interned,
            identity: bytes[9..].to_vec(),
        })
    }

    /// Returns the index name interned ID
    pub fn name_interned(&self) -> u64 {
        self.name_interned
    }

    /// Проверяет, что ключ соответствует указанному индексу
    pub fn matches_index(&self, name_interned: u64) -> bool {
        self.name_interned == name_interned
    }
}

/// Extracts the index-key prefix of a persisted posting key, given the
/// namespace byte. For a regular posting (`prefix || rid 16`) the prefix is
/// everything but the trailing 16 bytes; for a unique posting the whole key
/// IS the index key. Returns `None` for a key too short to be a v2 posting
/// of that namespace.
pub fn posting_index_prefix(key: &[u8]) -> Option<&[u8]> {
    match key.first().copied() {
        Some(REGULAR_V2) => key
            .len()
            .checked_sub(16)
            .filter(|&prefix_len| prefix_len >= 9)
            .map(|prefix_len| &key[..prefix_len]),
        Some(UNIQUE_V2) if key.len() >= 9 => Some(key),
        _ => None,
    }
}

/// True when `key` is a legacy-v1 base posting key (namespace `0x00`/`0x01`
/// with the fixed 25/41-byte layout). Used by the sweeps and the WAL-replay
/// cache invalidation, which must keep understanding v1 bytes that are still
/// in flight during a migration.
pub fn is_legacy_base_posting_key(key: &[u8]) -> bool {
    matches!(key.first().copied(), Some(0x00) | Some(0x01)) && key.len() >= 25
}
