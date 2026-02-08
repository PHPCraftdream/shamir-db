use serde::{Deserialize, Serialize};
use super::index_info_item::IndexInfoItem;

/// Defines a single index, which can be simple (one path) or composite (multiple paths).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDefinition {
    /// A unique name for the index.
    pub name: String,

    /// The list of paths that make up this index.
    /// A single path creates a simple index. Multiple paths create a composite index.
    pub paths: Vec<IndexInfoItem>,
}

impl IndexDefinition {
    pub fn new(name: &str, paths: Vec<IndexInfoItem>) -> Self {
        Self {
            name: name.to_string(),
            paths,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::bytes;

    #[test]
    fn test_index_definition_creation() {
        let def = IndexDefinition::new("by_email", vec![IndexInfoItem::new(vec![1])]);
        assert_eq!(def.name, "by_email");
        assert_eq!(def.paths.len(), 1);
    }

    #[test]
    fn test_index_definition_bincode() {
        let def = IndexDefinition::new("by_name", vec![IndexInfoItem::new(vec![1, 2])]);
        let serialized = bincode::serialize(&def).unwrap();
        let deserialized: IndexDefinition = bincode::deserialize(&serialized).unwrap();
        assert_eq!(def, deserialized);
    }

    #[test]
    fn test_index_definition_roundtrip() {
        let def = IndexDefinition::new(
            "composite_index",
            vec![
                IndexInfoItem::new(vec![1, 2]),
                IndexInfoItem::new(vec![3, 4, 5]),
            ],
        );

        let bytes = bytes::to_bytes(&def).unwrap();
        let deserialized: IndexDefinition = bytes::from_bytes(&bytes).unwrap();
        assert_eq!(def, deserialized);
    }

    #[test]
    fn test_index_definition_zero_copy() {
        let def = IndexDefinition::new(
            "test_index",
            vec![IndexInfoItem::new(vec![10, 20, 30])],
        );

        let bytes = bytes::to_bytes(&def).unwrap();
        let def2 = bytes::from_bytes::<IndexDefinition>(&bytes).unwrap();
        assert_eq!(def2.name, "test_index");
        assert_eq!(def2.paths.len(), 1);
        assert_eq!(&def2.paths[0].path[..], &[10, 20, 30]);
    }
}
