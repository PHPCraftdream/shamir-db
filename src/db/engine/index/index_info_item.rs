//! Index definition
//!
//! Defines a single index by path.

use serde::{Deserialize, Serialize};

/// Definition of a single index
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexInfoItem {
    /// Path to indexed field (interned components)
    pub path: Vec<u64>,
}

impl IndexInfoItem {
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
        let def = IndexInfoItem::new(vec![1, 2, 3]);
        assert_eq!(def.path, vec![1, 2, 3]);
    }

    #[test]
    fn test_index_def_serialization() {
        let def = IndexInfoItem {
            path: vec![1, 2, 3],
        };

        let serialized = bincode::serialize(&def).unwrap();
        let deserialized: IndexInfoItem = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized, def);
    }
}
