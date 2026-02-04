//! Index definition
//!
//! Defines a single index with path and uniqueness.

use serde::{Deserialize, Serialize};

/// Definition of a single index
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexDef {
    /// Path to indexed field (interned components)
    pub path: Vec<u64>,

    /// Is this a unique index?
    pub unique: bool,
}

impl IndexDef {
    /// Create a non-unique index
    pub fn new(path: Vec<u64>) -> Self {
        Self { path, unique: false }
    }

    /// Create a unique index
    pub fn unique(path: Vec<u64>) -> Self {
        Self { path, unique: true }
    }

    /// Check if this is a unique index
    pub fn is_unique(&self) -> bool {
        self.unique
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_def() {
        let def1 = IndexDef::new(vec![1, 2]);
        assert_eq!(def1.path, vec![1, 2]);
        assert!(!def1.is_unique());

        let def2 = IndexDef::unique(vec![3, 4]);
        assert_eq!(def2.path, vec![3, 4]);
        assert!(def2.is_unique());
    }

    #[test]
    fn test_index_def_serialization() {
        let def = IndexDef {
            path: vec![1, 2, 3],
            unique: true,
        };

        let serialized = bincode::serialize(&def).unwrap();
        let deserialized: IndexDef = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized, def);
    }
}
