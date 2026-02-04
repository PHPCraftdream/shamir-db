//! Index target configuration
//!
//! Defines what should be indexed for a table.

use serde::{Deserialize, Serialize};
use super::def::IndexDef;

/// Indexing target specification
///
/// Defines what should be indexed for a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexTarget {
    /// Indexing disabled - no indexing
    Disabled,

    /// Index everything - all Map fields are indexed (non-unique)
    All,

    /// Selective indexing - only specific indexes are active
    Selective(Vec<IndexDef>),
}

impl IndexTarget {
    /// Create Disabled target
    pub fn disabled() -> Self {
        IndexTarget::Disabled
    }

    /// Create All target
    pub fn all() -> Self {
        IndexTarget::All
    }

    /// Create Selective target with indexes
    pub fn selective(indexes: Vec<IndexDef>) -> Self {
        IndexTarget::Selective(indexes)
    }

    /// Check if indexing is enabled
    pub fn is_enabled(&self) -> bool {
        !matches!(self, IndexTarget::Disabled)
    }

    /// Check if all fields should be indexed
    pub fn is_all(&self) -> bool {
        matches!(self, IndexTarget::All)
    }

    /// Check if selective indexing is enabled
    pub fn is_selective(&self) -> bool {
        matches!(self, IndexTarget::Selective(_))
    }

    /// Add an index to selective mode
    /// If disabled, switches to Selective with single index
    /// If all, switches to Selective with single index
    pub fn add_index(&mut self, path: Vec<u64>, unique: bool) {
        match self {
            IndexTarget::Disabled => {
                *self = IndexTarget::Selective(vec![IndexDef { path, unique }]);
            }
            IndexTarget::All => {
                *self = IndexTarget::Selective(vec![IndexDef { path, unique }]);
            }
            IndexTarget::Selective(indexes) => {
                // Remove existing index with same path (if any)
                indexes.retain(|idx| idx.path != path);
                indexes.push(IndexDef { path, unique });
            }
        }
    }

    /// Remove an index from selective mode
    /// Returns true if index was removed, false if not found
    /// If indexes become empty, switches to Disabled
    pub fn remove_index(&mut self, path: &Vec<u64>) -> bool {
        let (should_disable, removed) = match self {
            IndexTarget::Disabled | IndexTarget::All => (false, false),
            IndexTarget::Selective(indexes) => {
                let was_present = indexes.iter().any(|idx| idx.path == *path);
                indexes.retain(|idx| idx.path != *path);
                (indexes.is_empty(), was_present)
            }
        };

        if should_disable {
            *self = IndexTarget::Disabled;
        }

        removed
    }

    /// Get all indexes if selective, None otherwise
    pub fn indexes(&self) -> Option<&[IndexDef]> {
        match self {
            IndexTarget::Selective(indexes) => Some(indexes),
            _ => None,
        }
    }

    /// Get only unique indexes
    pub fn unique_indexes(&self) -> Vec<IndexDef> {
        match self {
            IndexTarget::Selective(indexes) => {
                indexes.iter().filter(|idx| idx.unique).cloned().collect()
            }
            _ => Vec::new(),
        }
    }

    /// Check if a specific path is indexed (regardless of uniqueness)
    pub fn has_index(&self, path: &Vec<u64>) -> bool {
        match self {
            IndexTarget::All => true,
            IndexTarget::Disabled => false,
            IndexTarget::Selective(indexes) => {
                indexes.iter().any(|idx| idx.path == *path)
            }
        }
    }

    /// Check if a specific path has a unique index
    pub fn has_unique_index(&self, path: &Vec<u64>) -> bool {
        match self {
            IndexTarget::All => false,
            IndexTarget::Disabled => false,
            IndexTarget::Selective(indexes) => {
                indexes.iter().any(|idx| idx.path == *path && idx.unique)
            }
        }
    }
}

impl Default for IndexTarget {
    fn default() -> Self {
        Self::Disabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_target_disabled() {
        let target = IndexTarget::Disabled;
        assert!(!target.is_enabled());
        assert!(!target.is_all());
        assert!(!target.is_selective());
        assert!(target.indexes().is_none());
        assert!(target.unique_indexes().is_empty());
        assert!(!target.has_index(&vec![1]));
        assert!(!target.has_unique_index(&vec![1]));
    }

    #[test]
    fn test_index_target_all() {
        let target = IndexTarget::All;
        assert!(target.is_enabled());
        assert!(target.is_all());
        assert!(!target.is_selective());
        assert!(target.indexes().is_none());
        assert!(target.unique_indexes().is_empty());
        assert!(target.has_index(&vec![1]));
        assert!(!target.has_unique_index(&vec![1]));
    }

    #[test]
    fn test_index_target_selective() {
        let indexes = vec![
            IndexDef::new(vec![1, 2]),
            IndexDef::unique(vec![3, 4]),
        ];
        let target = IndexTarget::selective(indexes.clone());

        assert!(target.is_enabled());
        assert!(!target.is_all());
        assert!(target.is_selective());
        assert_eq!(target.indexes(), Some(&indexes as &[IndexDef]));
        assert_eq!(target.unique_indexes().len(), 1);
        assert!(target.has_index(&vec![1, 2]));
        assert!(target.has_unique_index(&vec![3, 4]));
    }

    #[test]
    fn test_index_target_add_index_to_disabled() {
        let mut target = IndexTarget::Disabled;
        target.add_index(vec![1, 2], false);

        assert!(target.is_selective());
        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef::new(vec![1, 2]));
    }

    #[test]
    fn test_index_target_add_unique_index_to_disabled() {
        let mut target = IndexTarget::Disabled;
        target.add_index(vec![1, 2], true);

        assert!(target.is_selective());
        assert_eq!(target.unique_indexes().len(), 1);
    }

    #[test]
    fn test_index_target_add_index_to_all() {
        let mut target = IndexTarget::All;
        target.add_index(vec![1, 2], false);

        assert!(target.is_selective());
        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef { path: vec![1, 2], unique: false });
    }

    #[test]
    fn test_index_target_add_index_to_selective() {
        let mut target = IndexTarget::selective(vec![IndexDef::new(vec![1])]);
        target.add_index(vec![2, 3], true);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 2);
        assert!(indexes.contains(&IndexDef::new(vec![1])));
        assert!(indexes.contains(&IndexDef::unique(vec![2, 3])));
    }

    #[test]
    fn test_index_target_remove_index_from_selective() {
        let mut target = IndexTarget::selective(vec![
            IndexDef::new(vec![1, 2]),
            IndexDef::unique(vec![3, 4]),
        ]);

        let removed = target.remove_index(&vec![1, 2]);
        assert!(removed);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef::unique(vec![3, 4]));
    }

    #[test]
    fn test_index_target_remove_last_index_becomes_disabled() {
        let mut target = IndexTarget::selective(vec![IndexDef::new(vec![1, 2])]);

        let removed = target.remove_index(&vec![1, 2]);
        assert!(removed);

        assert!(matches!(target, IndexTarget::Disabled));
        assert!(!target.is_enabled());
    }

    #[test]
    fn test_index_target_remove_unique_index_from_selective() {
        let mut target = IndexTarget::selective(vec![
            IndexDef::new(vec![1, 2]),
            IndexDef::unique(vec![3, 4]),
        ]);

        let removed = target.remove_index(&vec![3, 4]);
        assert!(removed);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef::new(vec![1, 2]));
        assert_eq!(target.unique_indexes().len(), 0);
    }

    #[test]
    fn test_index_target_remove_index_from_disabled() {
        let mut target = IndexTarget::Disabled;
        let removed = target.remove_index(&vec![1, 2]);

        assert!(!removed);
        assert!(matches!(target, IndexTarget::Disabled));
    }

    #[test]
    fn test_index_target_remove_index_from_all() {
        let mut target = IndexTarget::All;
        let removed = target.remove_index(&vec![1, 2]);

        assert!(!removed);
        assert!(matches!(target, IndexTarget::All));
    }

    #[test]
    fn test_index_target_serialization() {
        let targets = vec![
            IndexTarget::Disabled,
            IndexTarget::All,
            IndexTarget::selective(vec![
                IndexDef::new(vec![1, 2]),
                IndexDef::unique(vec![3]),
            ]),
        ];

        for original in targets {
            let serialized = bincode::serialize(&original).unwrap();
            let deserialized: IndexTarget = bincode::deserialize(&serialized).unwrap();
            assert_eq!(deserialized, original);
        }
    }

    #[test]
    fn test_index_target_has_unique_index() {
        let target = IndexTarget::selective(vec![
            IndexDef::new(vec![1]),
            IndexDef::unique(vec![2]),
        ]);

        assert!(!target.has_unique_index(&vec![1]));
        assert!(target.has_unique_index(&vec![2]));
        assert!(target.has_index(&vec![1]));
        assert!(target.has_index(&vec![2]));
    }

    #[test]
    fn test_index_target_default() {
        let target = IndexTarget::default();
        assert!(matches!(target, IndexTarget::Disabled));
    }
}
