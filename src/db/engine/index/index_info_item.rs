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
    use crate::codecs::bytes;

    #[test]
    fn test_index_def() {
        let def = IndexInfoItem::new(vec![1, 2, 3]);
        assert_eq!(def.path, vec![1, 2, 3]);
    }

    #[test]
    fn test_index_def_bincode() {
        let def = IndexInfoItem {
            path: vec![1, 2, 3],
        };

        let serialized = bincode::serialize(&def).unwrap();
        let deserialized: IndexInfoItem = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized, def);
    }

    #[test]
    fn test_index_def_roundtrip() {
        let def = IndexInfoItem {
            path: vec![1, 2, 3, 4, 5],
        };

        let bytes = bytes::to_bytes(&def).unwrap();
        let deserialized: IndexInfoItem = bytes::from_bytes(&bytes).unwrap();
        assert_eq!(deserialized, def);
    }

    #[test]
    fn test_index_def_bincode_roundtrip() {
        let def = IndexInfoItem {
            path: vec![10, 20, 30],
        };

        let bytes = bytes::to_bytes(&def).unwrap();
        let item2 = bytes::from_bytes::<IndexInfoItem>(&bytes).unwrap();
        assert_eq!(&item2.path[..], &[10, 20, 30]);
    }
}
