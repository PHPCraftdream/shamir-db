//! Index configuration and sync status

use super::index_definition::IndexDefinition;
use crate::index::index_status::IndexStatus;
use shamir_types::types::common::{new_dash_map_wc, TDashMap};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
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
/// For serialization, indexes are converted to/from BTreeMap.
#[derive(Debug, Clone)]
pub struct IndexInfo {
    indexes: TDashMap<u64, IndexDefinition>,
    status: StatusAtom,
}

impl Serialize for IndexInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Convert DashMap to BTreeMap for serialization
        let map: BTreeMap<u64, IndexDefinition> = self
            .indexes
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();
        map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IndexInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialize as BTreeMap, then convert to DashMap
        let map: BTreeMap<u64, IndexDefinition> = BTreeMap::deserialize(deserializer)?;
        let indexes = new_dash_map_wc(map.len().max(128));
        for (k, v) in map {
            indexes.insert(k, v);
        }
        Ok(Self {
            indexes,
            status: StatusAtom::default(),
        })
    }
}

impl IndexInfo {
    /// Create empty IndexInfo
    pub fn new() -> Self {
        Self {
            indexes: new_dash_map_wc(128),
            status: StatusAtom(Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8()))),
        }
    }

    /// Create IndexInfo with index definitions from iterator
    pub fn from_definitions<I: IntoIterator<Item = IndexDefinition>>(iter: I) -> Self {
        let items: Vec<_> = iter.into_iter().collect();
        let indexes = new_dash_map_wc(items.len().max(128));
        for def in items {
            indexes.insert(def.name_interned, def);
        }
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
    pub fn add_index(&self, index_def: IndexDefinition) {
        self.indexes.insert(index_def.name_interned, index_def);
        self.mark_pending();
    }

    /// Remove an index by its interned name.
    /// Returns true if an index was removed.
    pub fn remove_index(&self, name_interned: u64) -> bool {
        let removed = self.indexes.remove(&name_interned).is_some();
        if removed {
            self.mark_pending();
        }
        removed
    }

    /// Get the number of index definitions
    pub fn len(&self) -> usize {
        self.indexes.len()
    }

    /// Check if there are no indexes
    pub fn is_empty(&self) -> bool {
        self.indexes.is_empty()
    }

    /// Get an index definition by interned name
    pub fn get_index(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes.get(&name_interned).map(|v| v.clone())
    }

    /// Iterate over all index definitions
    pub fn iter(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes.iter().map(|entry| entry.value().clone())
    }

    /// Check if an index exists
    pub fn contains(&self, name_interned: u64) -> bool {
        self.indexes.contains_key(&name_interned)
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
        if self.indexes.len() != other.indexes.len() {
            return false;
        }
        for entry in self.indexes.iter() {
            let key = *entry.key();
            if let Some(other_val) = other.indexes.get(&key) {
                if entry.value() != other_val.value() {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

impl Eq for IndexInfo {}
