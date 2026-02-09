//! Index record key for B-Tree index
//!
//! The key consists of:
//! - is_unique flag (1 byte)
//! - number of paths (1 byte)
//! - each path: length (4 bytes) + path data (variable)
//! - hash1 (8 bytes)
//! - hash2 (8 bytes)

use bytes::Bytes;
use fxhash::FxHasher;
use std::hash::{Hash, Hasher};

/// Index record key for B-Tree
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecordKey {
    /// Whether this is a unique index (1) or not (0)
    pub is_unique: u8,

    /// Paths to indexed fields (vector of path components)
    /// Each path is a vector of field IDs (e.g., [1] for top-level field, [1, 2] for nested)
    pub path: Vec<Vec<u64>>,

    /// Hash of the indexed values
    pub hash1: u64,

    /// Second hash for collision resistance
    pub hash2: u64,
}

impl IndexRecordKey {
    /// Create a new index record key
    pub fn new(is_unique: bool, path: Vec<Vec<u64>>) -> Self {
        Self {
            is_unique: if is_unique { 1 } else { 0 },
            path,
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

        let path_hash: u64 = self
            .path
            .iter()
            .flat_map(|p| p.iter())
            .fold(0u64, |acc, &id| acc.wrapping_add(id));
        self.hash2 = self.hash1.wrapping_neg() ^ path_hash;

        self
    }

    /// Convert to bytes for storage
    pub fn to_bytes(&self) -> Bytes {
        let mut bytes = Vec::new();
        bytes.push(self.is_unique);
        bytes.push(self.path.len() as u8);

        for path in &self.path {
            bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
            bytes.extend_from_slice(
                &path
                    .iter()
                    .flat_map(|&id| id.to_le_bytes().to_vec())
                    .collect::<Vec<_>>(),
            );
        }

        bytes.extend_from_slice(&self.hash1.to_le_bytes());
        bytes.extend_from_slice(&self.hash2.to_le_bytes());

        Bytes::from(bytes)
    }

    /// Create from bytes
    pub fn from_bytes(bytes: Bytes) -> Result<Self, String> {
        if bytes.len() < 18 {
            return Err("IndexRecordKey too short".to_string());
        }

        let is_unique = bytes[0];
        let num_paths = bytes[1] as usize;
        let mut pos = 2;

        let mut path = Vec::with_capacity(num_paths);
        for _ in 0..num_paths {
            if pos + 4 > bytes.len() {
                return Err("IndexRecordKey: insufficient bytes for path length".to_string());
            }
            let path_len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;

            if pos + path_len * 8 > bytes.len() {
                return Err("IndexRecordKey: insufficient bytes for path data".to_string());
            }

            let path_vec: Vec<u64> = (0..path_len)
                .map(|i| {
                    u64::from_le_bytes(bytes[pos + i * 8..pos + (i + 1) * 8].try_into().unwrap())
                })
                .collect();
            pos += path_len * 8;

            path.push(path_vec);
        }

        if pos + 16 > bytes.len() {
            return Err("IndexRecordKey: insufficient bytes for hashes".to_string());
        }

        let hash1 = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        let hash2 = u64::from_le_bytes(bytes[pos + 8..pos + 16].try_into().unwrap());

        Ok(Self {
            is_unique,
            path,
            hash1,
            hash2,
        })
    }

    /// Возвращает ссылку на пути индекса.
    pub fn paths(&self) -> &[Vec<u64>] {
        &self.path
    }

    /// Проверяет, что ключ соответствует указанным путям
    pub fn matches_paths(&self, paths: &[Vec<u64>]) -> bool {
        if self.path.len() != paths.len() {
            return false;
        }
        self.path.iter().zip(paths.iter()).all(|(a, b)| a == b)
    }
}
