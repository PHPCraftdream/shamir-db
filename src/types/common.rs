use indexmap::{IndexMap, IndexSet};
use std::hash::{Hash, BuildHasherDefault};
use std::cmp::Eq;
use fxhash::FxHasher;
use dashmap::DashMap;

pub type THasher = BuildHasherDefault<FxHasher>;

// --- Collections with rkyv support ---
/// Ordered map that supports rkyv zero-copy deserialization
/// Maintains insertion order for predictable iteration
pub type TMap<K, V> = IndexMap<K, V, THasher>;

/// Ordered set that supports rkyv zero-copy deserialization
/// Maintains insertion order for predictable iteration
pub type TSet<T> = IndexSet<T, THasher>;

pub fn new_map<K: Eq + Hash, V>() -> TMap<K, V> {
    IndexMap::with_hasher(THasher::default())
}

pub fn new_map_wc<K: Eq + Hash, V>(capacity: usize) -> TMap<K, V> {
    IndexMap::with_capacity_and_hasher(capacity, THasher::default())
}

pub fn new_set<V: Eq + Hash>() -> TSet<V> {
    IndexSet::with_hasher(THasher::default())
}

pub fn new_set_wc<V: Eq + Hash>(capacity: usize) -> TSet<V> {
    IndexSet::with_capacity_and_hasher(capacity, THasher::default())
}

// --- Concurrent Collections (unchanged) ---
pub type TDashMap<K, V> = DashMap<K, V, THasher>;

pub fn new_dash_map<K: Eq + Hash, V>() -> TDashMap<K, V> {
    DashMap::with_hasher(THasher::default())
}

pub fn new_dash_map_wc<K: Eq + Hash, V>(capacity: usize) -> TDashMap<K, V> {
    DashMap::with_capacity_and_hasher(capacity, THasher::default())
}
