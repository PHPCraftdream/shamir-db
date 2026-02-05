//! Index target configuration
//!
//! Defines what should be indexed for a table and tracks sync status.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use super::index_info_item::IndexInfoItem;

/// Status of index synchronization with disk
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IndexStatus {
    /// Index matches disk state
    Actual = 0,
    /// Index was modified, needs to be saved
    Dirty = 1,
    /// Index is being saved to disk
    Saving = 2,
}

impl IndexStatus {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Actual,
            1 => Self::Dirty,
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

    /// Index everything - all Map fields are indexed
    All,

    /// Selective indexing - only specific paths are indexed
    Selective(Vec<IndexInfoItem>),
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

    /// Create Selective target with indexes
    pub fn selective(indexes: Vec<IndexInfoItem>) -> Self {
        Self {
            mode: IndexMode::Selective(indexes),
            status: Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8())),
        }
    }

    /// Check if indexing is enabled
    pub fn is_enabled(&self) -> bool {
        !matches!(self.mode, IndexMode::Disabled)
    }

    /// Check if all fields should be indexed
    pub fn is_all(&self) -> bool {
        matches!(self.mode, IndexMode::All)
    }

    /// Check if selective indexing is enabled
    pub fn is_selective(&self) -> bool {
        matches!(self.mode, IndexMode::Selective(_))
    }

    /// Get current status
    pub fn status(&self) -> IndexStatus {
        IndexStatus::from_u8(self.status.load(Ordering::Acquire))
    }

    /// Set status
    pub fn set_status(&self, status: IndexStatus) {
        self.status.store(status.as_u8(), Ordering::Release);
    }

    /// Mark as dirty (needs sync)
    pub fn mark_dirty(&self) {
        self.set_status(IndexStatus::Dirty);
    }

    /// Mark as actual (synced)
    pub fn mark_actual(&self) {
        self.set_status(IndexStatus::Actual);
    }

    /// Check if needs sync
    pub fn needs_sync(&self) -> bool {
        matches!(self.status(), IndexStatus::Dirty)
    }

    /// Add an index path
    pub fn add_index(&mut self, path: Vec<u64>) {
        match &mut self.mode {
            IndexMode::Disabled => {
                self.mode = IndexMode::Selective(vec![IndexInfoItem { path }]);
            }
            IndexMode::All => {
                // Already indexing everything, nothing to do
            }
            IndexMode::Selective(indexes) => {
                // Remove existing index with same path (if any)
                indexes.retain(|idx| idx.path != path);
                indexes.push(IndexInfoItem { path });
            }
        }
        self.mark_dirty();
    }

    /// Remove an index path
    /// Returns true if index was removed, false if not found
    pub fn remove_index(&mut self, path: &Vec<u64>) -> bool {
        let (should_disable, removed) = match &mut self.mode {
            IndexMode::Disabled | IndexMode::All => (false, false),
            IndexMode::Selective(indexes) => {
                let was_present = indexes.iter().any(|idx| idx.path == *path);
                indexes.retain(|idx| idx.path != *path);
                (indexes.is_empty(), was_present)
            }
        };

        if should_disable {
            self.mode = IndexMode::Disabled;
        }

        if removed {
            self.mark_dirty();
        }

        removed
    }

    /// Get all indexes if selective, None otherwise
    pub fn indexes(&self) -> Option<&[IndexInfoItem]> {
        match &self.mode {
            IndexMode::Selective(indexes) => Some(indexes),
            _ => None,
        }
    }

    /// Check if a specific path is indexed
    pub fn has_index(&self, path: &Vec<u64>) -> bool {
        match &self.mode {
            IndexMode::All => true,
            IndexMode::Disabled => false,
            IndexMode::Selective(indexes) => {
                indexes.iter().any(|idx| idx.path == *path)
            }
        }
    }


    /// Check if a specific path is indexed
    pub fn has_indexes(&self) -> bool {
        match &self.mode {
            IndexMode::All => true,
            IndexMode::Selective(v) => v.len() > 0,
            IndexMode::Disabled => false,
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

    #[test]
    fn test_index_target_disabled() {
        let target = IndexInfo::disabled();
        assert!(!target.is_enabled());
        assert!(!target.is_all());
        assert!(!target.is_selective());
        assert!(target.indexes().is_none());
        assert!(!target.has_index(&vec![1]));
        assert_eq!(target.status(), IndexStatus::Actual);
    }

    #[test]
    fn test_index_target_all() {
        let target = IndexInfo::all();
        assert!(target.is_enabled());
        assert!(target.is_all());
        assert!(!target.is_selective());
        assert!(target.indexes().is_none());
        assert!(target.has_index(&vec![1]));
    }

    #[test]
    fn test_index_target_selective() {
        let indexes = vec![
            IndexInfoItem::new(vec![1, 2]),
            IndexInfoItem::new(vec![3, 4]),
        ];
        let target = IndexInfo::selective(indexes.clone());

        assert!(target.is_enabled());
        assert!(!target.is_all());
        assert!(target.is_selective());
        assert_eq!(target.indexes(), Some(&indexes as &[IndexInfoItem]));
        assert!(target.has_index(&vec![1, 2]));
        assert!(target.has_index(&vec![3, 4]));
        assert!(!target.has_index(&vec![5]));
    }

    #[test]
    fn test_index_status() {
        let target = IndexInfo::all();
        assert_eq!(target.status(), IndexStatus::Actual);
        assert!(!target.needs_sync());

        target.mark_dirty();
        assert_eq!(target.status(), IndexStatus::Dirty);
        assert!(target.needs_sync());

        target.mark_actual();
        assert_eq!(target.status(), IndexStatus::Actual);
        assert!(!target.needs_sync());
    }

    #[test]
    fn test_add_index_marks_dirty() {
        let mut target = IndexInfo::disabled();
        assert!(!target.needs_sync());

        target.add_index(vec![1, 2]);
        assert!(target.needs_sync());
    }

    #[test]
    fn test_remove_index_marks_dirty() {
        let mut target = IndexInfo::selective(vec![IndexInfoItem::new(vec![1, 2])]);
        target.mark_actual(); // Clear initial dirty state

        target.remove_index(&vec![1, 2]);
        assert!(target.needs_sync());
    }

    #[test]
    fn test_index_target_add_index_to_disabled() {
        let mut target = IndexInfo::disabled();
        target.add_index(vec![1, 2]);

        assert!(target.is_selective());
        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].path, vec![1, 2]);
    }

    #[test]
    fn test_index_target_add_index_to_all() {
        let mut target = IndexInfo::all();
        target.add_index(vec![1, 2]);

        // Still All - no change
        assert!(target.is_all());
    }

    #[test]
    fn test_index_target_add_index_to_selective() {
        let mut target = IndexInfo::selective(vec![IndexInfoItem::new(vec![1])]);
        target.add_index(vec![2, 3]);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 2);
        assert!(indexes.iter().any(|idx| idx.path == vec![1]));
        assert!(indexes.iter().any(|idx| idx.path == vec![2, 3]));
    }

    #[test]
    fn test_index_target_add_duplicate_replaces() {
        let mut target = IndexInfo::selective(vec![IndexInfoItem::new(vec![1])]);
        target.add_index(vec![1]); // Add same path again

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1); // Only one entry
    }

    #[test]
    fn test_index_target_remove_index_from_selective() {
        let mut target = IndexInfo::selective(vec![
            IndexInfoItem::new(vec![1, 2]),
            IndexInfoItem::new(vec![3, 4]),
        ]);

        let removed = target.remove_index(&vec![1, 2]);
        assert!(removed);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].path, vec![3, 4]);
    }

    #[test]
    fn test_index_target_remove_last_index_becomes_disabled() {
        let mut target = IndexInfo::selective(vec![IndexInfoItem::new(vec![1, 2])]);

        let removed = target.remove_index(&vec![1, 2]);
        assert!(removed);

        assert!(!target.is_enabled());
    }

    #[test]
    fn test_index_target_remove_index_from_disabled() {
        let mut target = IndexInfo::disabled();
        let removed = target.remove_index(&vec![1, 2]);

        assert!(!removed);
        assert!(!target.is_enabled());
    }

    #[test]
    fn test_index_target_remove_index_from_all() {
        let mut target = IndexInfo::all();
        let removed = target.remove_index(&vec![1, 2]);

        assert!(!removed);
        assert!(target.is_all());
    }

    #[test]
    fn test_index_target_serialization() {
        let targets = vec![
            IndexInfo::disabled(),
            IndexInfo::all(),
            IndexInfo::selective(vec![
                IndexInfoItem::new(vec![1, 2]),
                IndexInfoItem::new(vec![3]),
            ]),
        ];

        for original in targets {
            let serialized = bincode::serialize(&original).unwrap();
            let deserialized: IndexInfo = bincode::deserialize(&serialized).unwrap();
            assert_eq!(deserialized, original);
            // Status should be Actual after deserialization (default)
            assert_eq!(deserialized.status(), IndexStatus::Actual);
        }
    }

    #[test]
    fn test_index_target_default() {
        let target = IndexInfo::default();
        assert!(!target.is_enabled());
    }

    #[test]
    fn test_status_not_serialized() {
        let target = IndexInfo::all();
        target.mark_dirty();

        let serialized = bincode::serialize(&target).unwrap();
        let deserialized: IndexInfo = bincode::deserialize(&serialized).unwrap();

        // Status is reset to Actual after deserialization
        assert_eq!(deserialized.status(), IndexStatus::Actual);
    }
}
