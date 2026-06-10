use super::index_info_item::IndexInfoItem;
use serde::{Deserialize, Serialize};

/// Defines a single index, which can be simple (one path) or composite (multiple paths).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDefinition {
    /// Interned ID of the index name (used in IndexRecordKey)
    pub name_interned: u64,

    /// The list of paths that make up this index.
    /// A single path creates a simple index. Multiple paths create a composite index.
    pub paths: Vec<IndexInfoItem>,
}

impl IndexDefinition {
    pub fn new(name_interned: u64, paths: Vec<IndexInfoItem>) -> Self {
        Self {
            name_interned,
            paths,
        }
    }
}
