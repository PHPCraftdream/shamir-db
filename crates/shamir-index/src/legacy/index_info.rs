//! Index configuration and sync status

use super::index_definition::IndexDefinition;
use crate::legacy::index_status::IndexStatus;
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
/// Definitions live in a `NodeReplicated<Vec<IndexDefinition>>` — a
/// NUMA-aware, read-mostly RCU-style snapshot. Reads
/// (`iter`/`get_index`/`contains`/`len`) are lock-free against the calling
/// thread's node-local `Arc<Vec<...>>` replica; writes
/// (`add_index`/`remove_index`) copy-on-write and mirror to all node
/// replicas. On single-socket machines (dev, Windows, CI) there is exactly
/// one replica, giving identical performance to a bare `ArcSwap`. On
/// multi-socket NUMA machines each node reads its own replica without
/// crossing a socket interconnect. Replaces the previous per-node
/// `ArcSwap<Vec<…>>` (post-#292) with per-node replication (NUMA N3 step).
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
pub struct IndexInfo {
    indexes: shamir_numa::NodeReplicated<Vec<IndexDefinition>>,
    status: StatusAtom,
}

impl std::fmt::Debug for IndexInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.indexes.load_local();
        f.debug_struct("IndexInfo")
            .field("indexes", &*snap)
            .field("status", &self.status)
            .finish()
    }
}

impl Serialize for IndexInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Convert snapshot to BTreeMap for stable on-disk order.
        // Arc::clone extends the snapshot lifetime beyond the Guard.
        let snap = Arc::clone(&*self.indexes.load_local());
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
            indexes: shamir_numa::NodeReplicated::new(shamir_numa::detect(), vec),
            status: StatusAtom::default(),
        })
    }
}

impl IndexInfo {
    /// Create empty IndexInfo
    pub fn new() -> Self {
        Self {
            indexes: shamir_numa::NodeReplicated::new(shamir_numa::detect(), Vec::new()),
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
            indexes: shamir_numa::NodeReplicated::new(shamir_numa::detect(), vec),
            status: StatusAtom(Arc::new(AtomicU8::new(IndexStatus::Actual.as_u8()))),
        }
    }

    /// Check if indexing is enabled
    pub fn is_enabled(&self) -> bool {
        !self.indexes.load_local().is_empty()
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
    /// Uses `NodeReplicated::rcu` so concurrent `add_index`/`remove_index`
    /// callers retry their COW pass instead of overwriting each other, and
    /// the winning value is mirrored to all per-node replicas.
    pub fn add_index(&self, index_def: IndexDefinition) {
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<IndexDefinition> = (*cur).clone();
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
        self.indexes.load_local().len()
    }

    /// Check if there are no indexes
    pub fn is_empty(&self) -> bool {
        self.indexes.load_local().is_empty()
    }

    /// Get an index definition by interned name
    pub fn get_index(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes
            .load_local()
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
        // Arc::clone extends the snapshot lifetime past the Guard's scope.
        let snap = Arc::clone(&*self.indexes.load_local());
        let len = snap.len();
        (0..len).map(move |i| snap[i].clone())
    }

    /// Check if an index exists
    pub fn contains(&self, name_interned: u64) -> bool {
        self.indexes
            .load_local()
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
        // NodeReplicated is not Clone (owns per-node resources). Build a fresh
        // instance from the current snapshot: take a Vec clone from the local
        // replica and hand it to a new NodeReplicated. Subsequent writes
        // through either instance COW-replace their own replicas independently.
        let snap = Arc::clone(&*self.indexes.load_local());
        Self {
            indexes: shamir_numa::NodeReplicated::new(shamir_numa::detect(), (*snap).clone()),
            status: self.status.clone(),
        }
    }
}

// PartialEq based on indexes only (status is runtime state).
// Compares as a set keyed by `name_interned`, matching the previous
// DashMap-based semantics where insertion order didn't matter.
impl PartialEq for IndexInfo {
    fn eq(&self, other: &Self) -> bool {
        let a = self.indexes.load_local();
        let b = other.indexes.load_local();
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
