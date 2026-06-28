//! Index configuration and sync status

use super::index_definition::IndexDefinition;
use crate::legacy::index_status::IndexStatus;
use arc_swap::ArcSwap;
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
/// # Storage
///
/// Definitions live in `ArcSwap<Vec<IndexDefinition>>` — a read-mostly
/// RCU-style snapshot. Reads (`iter`/`get_index`/`contains`/`len`) are
/// lock-free against an `Arc<Vec<...>>` snapshot; writes
/// (`add_index`/`remove_index`) copy-on-write. Replaces the previous
/// sharded `DashMap` whose per-shard read-locks dominated hot-path
/// planner iteration (post-#290 flamegraph: residual ~3.1% `dashmap::Iter`).
///
/// # Cardinality assumption
///
/// Typical workloads have ≤ ~10 indexes per table, so linear scan over
/// the Vec is cache-friendly and beats `HashMap::get` (~5 ns vs ~30-80 ns)
/// without hashing the key.
///
/// # Serialization
///
/// Status is NOT serialized — runtime-only state. Definitions are
/// converted to/from `BTreeMap<u64, IndexDefinition>` for stable on-disk
/// order (keyed by `name_interned`).
#[derive(Debug)]
pub struct IndexInfo {
    indexes: ArcSwap<Vec<IndexDefinition>>,
    status: StatusAtom,
}

impl Serialize for IndexInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Convert snapshot to BTreeMap for stable on-disk order.
        let snap = self.indexes.load_full();
        let map: BTreeMap<u64, IndexDefinition> = snap
            .iter()
            .map(|def| (def.name_interned, def.clone()))
            .collect();
        map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IndexInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // BTreeMap → Vec; on-disk order preserved by BTreeMap's key ordering.
        let map: BTreeMap<u64, IndexDefinition> = BTreeMap::deserialize(deserializer)?;
        let vec: Vec<IndexDefinition> = map.into_values().collect();
        Ok(Self {
            indexes: ArcSwap::from_pointee(vec),
            status: StatusAtom::default(),
        })
    }
}

impl IndexInfo {
    /// Create empty IndexInfo
    pub fn new() -> Self {
        Self {
            indexes: ArcSwap::from_pointee(Vec::new()),
            status: StatusAtom(Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8()))),
        }
    }

    /// Create IndexInfo with index definitions from iterator.
    /// Duplicates by `name_interned` resolve last-wins (matches the previous
    /// DashMap-based behavior).
    pub fn from_definitions<I: IntoIterator<Item = IndexDefinition>>(iter: I) -> Self {
        let mut dedup: BTreeMap<u64, IndexDefinition> = BTreeMap::new();
        for def in iter {
            dedup.insert(def.name_interned, def);
        }
        let vec: Vec<IndexDefinition> = dedup.into_values().collect();
        Self {
            indexes: ArcSwap::from_pointee(vec),
            status: StatusAtom(Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8()))),
        }
    }

    /// Check if indexing is enabled
    pub fn is_enabled(&self) -> bool {
        !self.indexes.load().is_empty()
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

    /// Add or update an index definition (copy-on-write under a CAS loop).
    ///
    /// If a definition with the same `name_interned` exists, it is replaced
    /// in-place; otherwise the new definition is appended. Last-write-wins
    /// matches the previous `DashMap::insert` semantics.
    ///
    /// Uses `ArcSwap::rcu` so concurrent `add_index`/`remove_index` callers
    /// retry their COW pass instead of overwriting each other.
    pub fn add_index(&self, index_def: IndexDefinition) {
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<IndexDefinition> = (**cur).clone();
            match new_vec
                .iter()
                .position(|d| d.name_interned == index_def.name_interned)
            {
                Some(pos) => new_vec[pos] = index_def.clone(),
                None => new_vec.push(index_def.clone()),
            }
            new_vec
        });
        self.mark_pending();
    }

    /// Remove an index by its interned name (copy-on-write under a CAS loop).
    /// Returns true if an index was removed.
    pub fn remove_index(&self, name_interned: u64) -> bool {
        let mut removed = false;
        self.indexes.rcu(|cur| {
            let initial_len = cur.len();
            let new_vec: Vec<IndexDefinition> = cur
                .iter()
                .filter(|d| d.name_interned != name_interned)
                .cloned()
                .collect();
            removed = new_vec.len() != initial_len;
            new_vec
        });
        if removed {
            self.mark_pending();
        }
        removed
    }

    /// Get the number of index definitions
    pub fn len(&self) -> usize {
        self.indexes.load().len()
    }

    /// Check if there are no indexes
    pub fn is_empty(&self) -> bool {
        self.indexes.load().is_empty()
    }

    /// Get an index definition by interned name
    pub fn get_index(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes
            .load()
            .iter()
            .find(|d| d.name_interned == name_interned)
            .cloned()
    }

    /// Iterate over all index definitions.
    ///
    /// Returns an owned iterator over a snapshot of the current registry.
    /// The snapshot Arc is held by the returned iterator so concurrent
    /// writers' COW replacements don't disturb the iteration.
    pub fn iter(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        let snap = self.indexes.load_full();
        let len = snap.len();
        (0..len).map(move |i| snap[i].clone())
    }

    /// Check if an index exists
    pub fn contains(&self, name_interned: u64) -> bool {
        self.indexes
            .load()
            .iter()
            .any(|d| d.name_interned == name_interned)
    }
}

impl Default for IndexInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for IndexInfo {
    fn clone(&self) -> Self {
        // Share the same Arc snapshot — both clones see the current state.
        // Subsequent writes through either clone's `add_index`/`remove_index`
        // COW-replace the Arc, independent of the other.
        Self {
            indexes: ArcSwap::from(self.indexes.load_full()),
            status: self.status.clone(),
        }
    }
}

// PartialEq based on indexes only (status is runtime state).
// Compares as a set keyed by `name_interned`, matching the previous
// DashMap-based semantics where insertion order didn't matter.
impl PartialEq for IndexInfo {
    fn eq(&self, other: &Self) -> bool {
        let a = self.indexes.load();
        let b = other.indexes.load();
        if a.len() != b.len() {
            return false;
        }
        for def_a in a.iter() {
            match b.iter().find(|d| d.name_interned == def_a.name_interned) {
                Some(d) if d == def_a => continue,
                _ => return false,
            }
        }
        true
    }
}

impl Eq for IndexInfo {}
