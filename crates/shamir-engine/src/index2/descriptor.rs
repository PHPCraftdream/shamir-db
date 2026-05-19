//! Persisted description of a single index instance.

use crate::index2::kind::IndexKind;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDescriptor {
    /// Compact numeric ID used as the posting-key prefix (4 bytes,
    /// auto-incremented by `IndexRegistry`). Identifies the index in
    /// the hot path.
    pub id: u32,
    /// Human-readable name (DDL surface, error messages).
    pub name: String,
    /// Interned name — used for in-memory dispatch where strings are
    /// expensive (cross-references with `Interner`).
    pub name_interned: u64,
    /// Field paths the index covers. Each path is `Vec<u64>` of
    /// interned segment keys. SmallVec inline cap = 2 (most indexes
    /// are single- or two-field).
    pub paths: SmallVec<[Vec<u64>; 2]>,
    pub kind: IndexKind,
    pub created_at_nanos: u64,
    /// Opaque backend-specific tuning (bincode-friendly). Encoded
    /// JSON or whatever the backend wants; empty by default.
    #[serde(default)]
    pub options: Vec<u8>,
}

impl IndexDescriptor {
    pub fn new(
        id: u32,
        name: impl Into<String>,
        name_interned: u64,
        paths: SmallVec<[Vec<u64>; 2]>,
        kind: IndexKind,
    ) -> Self {
        let created_at_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            id,
            name: name.into(),
            name_interned,
            paths,
            kind,
            created_at_nanos,
            options: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_round_trip() {
        let mut paths: SmallVec<[Vec<u64>; 2]> = SmallVec::new();
        paths.push(vec![1, 2, 3]);
        let d = IndexDescriptor::new(7, "users_email", 42, paths, IndexKind::Btree { unique: true });
        let bytes = bincode::serialize(&d).unwrap();
        let got: IndexDescriptor = bincode::deserialize(&bytes).unwrap();
        assert_eq!(got.id, 7);
        assert_eq!(got.name, "users_email");
        assert_eq!(got.name_interned, 42);
        assert_eq!(got.paths[0], vec![1, 2, 3]);
    }
}
