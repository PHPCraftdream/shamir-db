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
use fxhash::FxHasher;
use std::hash::{Hash, Hasher};

/// Index record key for B-Tree
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecordKey {
    /// Whether this is a unique index (1) or not (0)
    pub is_unique: u8,

    /// Interned ID of the index name (uniquely identifies the index and its fields)
    pub index_name_interned: u64,

    /// Hash of the indexed values
    pub hash1: u64,

    /// Second hash for collision resistance
    pub hash2: u64,
}

impl IndexRecordKey {
    /// Create a new index record key
    pub fn new(is_unique: bool, index_name_interned: u64) -> Self {
        Self {
            is_unique: if is_unique { 1 } else { 0 },
            index_name_interned,
            hash1: 0,
            hash2: 0,
        }
    }

    /// Set hash values from the provided values
    pub fn with_values<T: Hash>(mut self, values: &[&T]) -> Self {
        let mut hasher = FxHasher::default();
        for value in values {
            value.hash(&mut hasher);
        }
        self.hash1 = hasher.finish();

        // Mix index_name_interned into hash2 for additional uniqueness
        self.hash2 = self.hash1.wrapping_neg() ^ self.index_name_interned;

        self
    }

    /// Convert to bytes for storage
    pub fn to_bytes(&self) -> Bytes {
        let mut bytes = [0u8; 25];
        bytes[0] = self.is_unique;
        bytes[1..9].copy_from_slice(&self.index_name_interned.to_le_bytes());
        bytes[9..17].copy_from_slice(&self.hash1.to_le_bytes());
        bytes[17..25].copy_from_slice(&self.hash2.to_le_bytes());

        Bytes::copy_from_slice(&bytes)
    }

    /// Convert to prefix bytes (without hash values) for scanning
    pub fn to_prefix_bytes(&self) -> Bytes {
        let mut bytes = [0u8; 9];
        bytes[0] = self.is_unique;
        bytes[1..9].copy_from_slice(&self.index_name_interned.to_le_bytes());

        Bytes::copy_from_slice(&bytes)
    }

    /// Create from bytes
    pub fn from_bytes(bytes: Bytes) -> Result<Self, String> {
        if bytes.len() < 25 {
            return Err("IndexRecordKey too short".to_string());
        }

        let is_unique = bytes[0];
        let index_name_interned = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
        let hash1 = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
        let hash2 = u64::from_le_bytes(bytes[17..25].try_into().unwrap());

        Ok(Self {
            is_unique,
            index_name_interned,
            hash1,
            hash2,
        })
    }

    /// Returns the index name interned ID
    pub fn index_name_interned(&self) -> u64 {
        self.index_name_interned
    }

    /// Проверяет, что ключ соответствует указанному индексу
    pub fn matches_index(&self, index_name_interned: u64) -> bool {
        self.index_name_interned == index_name_interned
    }
}
