//! Index target configuration
//!
//! Defines what should be indexed for a table and tracks sync status.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use super::index_definition::IndexDefinition;

/// Status of index synchronization with disk
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IndexStatus {
    /// Index matches disk state
    Actual = 0,
    /// Index was modified, needs to be saved
    Pending = 1,
    /// Index is being saved to disk
    Saving = 2,
}

impl IndexStatus {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Actual,
            1 => Self::Pending,
            _ => Self::Saving,
        }
    }

    fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Indexing mode - what to index
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexMode {
    /// Indexing disabled
    Disabled,

    /// Index everything - all Map fields are indexed (simple indexes only)
    All,

    /// Selective indexing - only specific indexes are created
    Selective(Vec<IndexDefinition>),
}

/// Indexing target with mode and sync status
///
/// Status is NOT serialized - it's runtime-only state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    mode: IndexMode,
    /// Status is skipped during serialization
    #[serde(skip)]
    status: Arc<AtomicU8>,
}

impl IndexInfo {
    /// Create Disabled target
    pub fn disabled() -> Self {
        Self {
            mode: IndexMode::Disabled,
            status: Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8())),
        }
    }

    /// Create All target
    pub fn all() -> Self {
        Self {
            mode: IndexMode::All,
            status: Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8())),
        }
    }

    /// Create Selective target with a list of index definitions
    pub fn selective(indexes: Vec<IndexDefinition>) -> Self {
        Self {
            mode: IndexMode::Selective(indexes),
            status: Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8())),
        }
    }

    /// Check if indexing is enabled
    pub fn is_enabled(&self) -> bool {
        !matches!(self.mode, IndexMode::Disabled)
    }

    /// Get current status
    pub fn status(&self) -> IndexStatus {
        IndexStatus::from_u8(self.status.load(Ordering::Acquire))
    }

    /// Set status
    pub fn set_status(&self, status: IndexStatus) {
        self.status.store(status.as_u8(), Ordering::Release);
    }

    /// Mark as pending (needs sync)
    pub fn mark_pending(&self) {
        self.set_status(IndexStatus::Pending);
    }

    /// Add or update an index definition.
    pub fn add_index(&mut self, index_def: IndexDefinition) {
        match &mut self.mode {
            IndexMode::Disabled => {
                self.mode = IndexMode::Selective(vec![index_def]);
            }
            IndexMode::All => {
                // Cannot add custom indexes in 'All' mode.
            }
            IndexMode::Selective(indexes) => {
                // Remove existing index with same name (if any) to replace it
                indexes.retain(|idx| idx.name != index_def.name);
                indexes.push(index_def);
            }
        }
        self.mark_pending();
    }

    /// Remove an index by its name.
    /// Returns true if an index was removed.
    pub fn remove_index(&mut self, name: &str) -> bool {
        let (should_disable, removed) = match &mut self.mode {
            IndexMode::Disabled | IndexMode::All => (false, false),
            IndexMode::Selective(indexes) => {
                let initial_len = indexes.len();
                indexes.retain(|idx| idx.name != name);
                let removed = indexes.len() < initial_len;
                (indexes.is_empty(), removed)
            }
        };

        if should_disable {
            self.mode = IndexMode::Disabled;
        }

        if removed {
            self.mark_pending();
        }
        removed
    }

    /// Get all index definitions if in selective mode.
    pub fn definitions(&self) -> Option<&[IndexDefinition]> {
        match &self.mode {
            IndexMode::Selective(definitions) => Some(definitions),
            _ => None,
        }
    }
}

impl Default for IndexInfo {
    fn default() -> Self {
        Self::disabled()
    }
}

// PartialEq based on mode only (status is runtime state)
impl PartialEq for IndexInfo {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
    }
}

impl Eq for IndexInfo {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::engine::index::index_info_item::IndexInfoItem;

    #[test]
    fn test_selective_mode_with_definitions() {
        let simple_index = IndexDefinition::new("by_email", vec![IndexInfoItem::new(vec![1])]);
        let composite_index = IndexDefinition::new("by_city_and_age", vec![IndexInfoItem::new(vec![2]), IndexInfoItem::new(vec![3])]);

        let mut target = IndexInfo::selective(vec![simple_index.clone()]);
        assert!(target.is_enabled());
        assert_eq!(target.definitions().unwrap().len(), 1);

        target.add_index(composite_index.clone());
        assert_eq!(target.definitions().unwrap().len(), 2);
        assert!(target.definitions().unwrap().contains(&simple_index));
        assert!(target.definitions().unwrap().contains(&composite_index));
    }

    #[test]
    fn test_add_and_remove_index() {
        let mut target = IndexInfo::disabled();
        let index1 = IndexDefinition::new("by_name", vec![IndexInfoItem::new(vec![1])]);
        let index2 = IndexDefinition::new("by_age", vec![IndexInfoItem::new(vec![2])]);

        target.add_index(index1.clone());
        assert!(matches!(target.mode, IndexMode::Selective(_)));
        assert_eq!(target.definitions().unwrap().len(), 1);

        target.add_index(index2.clone());
        assert_eq!(target.definitions().unwrap().len(), 2);

        // Test removing an index
        assert!(target.remove_index("by_name"));
        assert_eq!(target.definitions().unwrap().len(), 1);
        assert_eq!(target.definitions().unwrap()[0], index2);

        // Test removing the last index
        assert!(target.remove_index("by_age"));
        assert!(matches!(target.mode, IndexMode::Disabled));
        assert!(!target.is_enabled());
    }

    #[test]
    fn test_add_duplicate_name_replaces() {
        let mut target = IndexInfo::selective(vec![IndexDefinition::new("other", vec![])]);
        let index_v1 = IndexDefinition::new("my_index", vec![IndexInfoItem::new(vec![1])]);
        let index_v2 = IndexDefinition::new("my_index", vec![IndexInfoItem::new(vec![2])]);

        target.add_index(index_v1);
        assert_eq!(target.definitions().unwrap().len(), 2);
        assert_ne!(target.definitions().unwrap()[1], index_v2);

        target.add_index(index_v2.clone());
        assert_eq!(target.definitions().unwrap().len(), 2);
        assert_eq!(target.definitions().unwrap()[1], index_v2);
    }

    #[test]
    fn test_serialization() {
        let index_def = IndexDefinition::new("by_email", vec![IndexInfoItem::new(vec![1])]);
        let target = IndexInfo::selective(vec![index_def]);
        target.mark_pending();

        let serialized = bincode::serialize(&target).unwrap();
        let deserialized: IndexInfo = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.mode, target.mode);
        // Status is not serialized and should be reset to default (Actual)
        assert_eq!(deserialized.status(), IndexStatus::Actual);
    }
}
