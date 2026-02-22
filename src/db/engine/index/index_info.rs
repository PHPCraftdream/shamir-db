//! Index configuration and sync status

use super::index_definition::IndexDefinition;
use crate::db::engine::index::index_status::IndexStatus;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Wrapper around Arc<AtomicU8> that implements Default for deserialization.
/// The default value is IndexStatus::Actual.
#[derive(Debug, Clone)]
struct StatusAtom(Arc<AtomicU8>);

impl Default for StatusAtom {
    fn default() -> Self {
        Self(Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8())))
    }
}

impl std::ops::Deref for StatusAtom {
    type Target = Arc<AtomicU8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Index configuration with list of index definitions and sync status
///
/// Status is NOT serialized - it's runtime-only state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    indexes: Vec<IndexDefinition>,
    #[serde(skip)]
    status: StatusAtom,
}

impl IndexInfo {
    /// Create empty IndexInfo
    pub fn new() -> Self {
        Self {
            indexes: Vec::new(),
            status: StatusAtom(Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8()))),
        }
    }

    /// Create IndexInfo with index definitions
    pub fn from_definitions(indexes: Vec<IndexDefinition>) -> Self {
        Self {
            indexes,
            status: StatusAtom(Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8()))),
        }
    }

    /// Check if indexing is enabled
    pub fn is_enabled(&self) -> bool {
        !self.indexes.is_empty()
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

    /// Add or update an index definition
    pub fn add_index(&mut self, index_def: IndexDefinition) {
        self.indexes.retain(|idx| idx.name != index_def.name);
        self.indexes.push(index_def);
        self.mark_pending();
    }

    /// Remove an index by its name.
    /// Returns true if an index was removed.
    pub fn remove_index(&mut self, name: &str) -> bool {
        let initial_len = self.indexes.len();
        self.indexes.retain(|idx| idx.name != name);
        let removed = self.indexes.len() < initial_len;
        if removed {
            self.mark_pending();
        }
        removed
    }

    /// Get all index definitions
    pub fn definitions(&self) -> &[IndexDefinition] {
        &self.indexes
    }

    /// Get an index definition by name
    pub fn get_index(&self, name: &str) -> Option<&IndexDefinition> {
        self.indexes.iter().find(|idx| idx.name == name)
    }

    /// Get mutable reference to indexes
    pub fn definitions_mut(&mut self) -> &mut Vec<IndexDefinition> {
        &mut self.indexes
    }
}

impl Default for IndexInfo {
    fn default() -> Self {
        Self::new()
    }
}

// PartialEq based on indexes only (status is runtime state)
impl PartialEq for IndexInfo {
    fn eq(&self, other: &Self) -> bool {
        self.indexes == other.indexes
    }
}

impl Eq for IndexInfo {}
