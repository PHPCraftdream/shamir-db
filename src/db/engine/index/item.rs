//! Index definition
//!
//! Defines a single index by path.

use serde::{Deserialize, Serialize};

/// Definition of a single index
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexItem {
    /// Path to indexed field (interned components)
    pub path: Vec<u64>,
}

impl IndexItem {
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
        let def = IndexItem::new(vec![1, 2, 3]);
        assert_eq!(def.path, vec![1, 2, 3]);
    }

    #[test]
    fn test_index_def_serialization() {
        let def = IndexItem {
            path: vec![1, 2, 3],
        };

        let serialized = bincode::serialize(&def).unwrap();
        let deserialized: IndexItem = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized, def);
    }
}
