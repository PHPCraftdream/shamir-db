//! Index definition
//!
//! Defines a single index by path.

use serde::{Deserialize, Serialize};

/// Definition of a single index
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexDef {
    /// Path to indexed field (interned components)
    pub path: Vec<u64>,
}

impl IndexDef {
    /// Create an index definition
    pub fn new(path: Vec<u64>) -> Self {
        Self { path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_def() {
        let def = IndexDef::new(vec![1, 2, 3]);
        assert_eq!(def.path, vec![1, 2, 3]);
    }

    #[test]
    fn test_index_def_serialization() {
        let def = IndexDef {
            path: vec![1, 2, 3],
        };

        let serialized = bincode::serialize(&def).unwrap();
        let deserialized: IndexDef = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized, def);
    }
}
