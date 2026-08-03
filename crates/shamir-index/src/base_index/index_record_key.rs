//! Index record key for B-Tree index
//!
//! The key consists of:
//! - is_unique flag (1 byte)
//! - index_id (8 bytes) - interned ID of the index name
//! - hash1 (8 bytes)
//! - hash2 (8 bytes)
//!
//! Total: 25 bytes

use bytes::Bytes;
use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};

/// Index record key for B-Tree
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecordKey {
    /// Whether this is a unique index (1) or not (0)
    pub is_unique: u8,

    /// Interned ID of the index name (uniquely identifies the index and its fields)
    pub name_interned: u64,

    /// Hash of the indexed values
    pub hash1: u64,

    /// Second hash for collision resistance
    pub hash2: u64,
}

impl IndexRecordKey {
    /// Create a new index record key
    pub fn new(is_unique: bool, name_interned: u64) -> Self {
        Self {
            is_unique: if is_unique { 1 } else { 0 },
            name_interned,
            hash1: 0,
            hash2: 0,
        }
    }

    /// Set hash values from pre-computed hashes.
    ///
    /// This is the primary constructor for S9+ (lens-native hashing):
    /// the caller computes `(hash1, hash2)` via the stable tag-based
    /// `hash_scalar_ref` / `hash_inner_value` scheme in `index_keys.rs`
    /// and passes the results here.
    pub fn with_hash(mut self, hash1: u64, hash2: u64) -> Self {
        self.hash1 = hash1;
        self.hash2 = hash2;
        self
    }

    /// Set hash values from the provided values using FxHasher with different seeds
    /// for collision resistance:
    /// - hash1: FxHasher with seed 0
    /// - hash2: FxHasher with seed 0x9E3779B97F4A7C15 (golden ratio constant)
    ///
    /// **DEPRECATED (S9)**: retained for `index_record_key_tests` only.
    /// Production write/lookup paths use `with_hash` + the stable tag-based
    /// hash in `index_keys.rs`.
    pub fn with_values<T: Hash>(mut self, values: &[&T]) -> Self {
        const SEED2: u64 = 0x9E3779B97F4A7C15;

        let mut hasher1 = FxHasher::default();
        let mut hasher2 = FxHasher::default();

        SEED2.hash(&mut hasher2);
        self.name_interned.hash(&mut hasher2);
        self.name_interned.hash(&mut hasher1);

        for value in values {
            value.hash(&mut hasher1);
            value.hash(&mut hasher2);
        }

        self.hash1 = hasher1.finish();
        self.hash2 = hasher2.finish();

        self
    }

    /// Convert to bytes for storage
    pub fn to_bytes(&self) -> Bytes {
        let mut bytes = [0u8; 25];
        bytes[0] = self.is_unique;
        bytes[1..9].copy_from_slice(&self.name_interned.to_le_bytes());
        bytes[9..17].copy_from_slice(&self.hash1.to_le_bytes());
        bytes[17..25].copy_from_slice(&self.hash2.to_le_bytes());

        Bytes::copy_from_slice(&bytes)
    }

    /// Convert to prefix bytes (without hash values) for scanning
    pub fn to_prefix_bytes(&self) -> Bytes {
        let mut bytes = [0u8; 9];
        bytes[0] = self.is_unique;
        bytes[1..9].copy_from_slice(&self.name_interned.to_le_bytes());

        Bytes::copy_from_slice(&bytes)
    }

    /// Create from bytes
    pub fn from_bytes(bytes: Bytes) -> Result<Self, String> {
        if bytes.len() < 25 {
            return Err("IndexRecordKey too short".to_string());
        }

        let is_unique = bytes[0];
        let name_interned = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
        let hash1 = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
        let hash2 = u64::from_le_bytes(bytes[17..25].try_into().unwrap());

        Ok(Self {
            is_unique,
            name_interned,
            hash1,
            hash2,
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
