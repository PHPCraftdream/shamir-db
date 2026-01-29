use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash};
use std::cmp::Eq;
use fxhash::FxHasher;
use dashmap::DashMap;

pub type THasher = BuildHasherDefault<FxHasher>;

// --- Standard Collections ---
pub type TSet<T> = HashSet<T, THasher>;
pub type TMap<K, V> = HashMap<K, V, THasher>;

pub fn new_map<K: Eq + Hash, V>() -> TMap<K, V> {
    TMap::with_hasher(THasher::default())
}

pub fn new_map_wc<K: Eq + Hash, V>(capacity: usize) -> TMap<K, V> {
    TMap::with_capacity_and_hasher(capacity, THasher::default())
}

pub fn new_set<V: Eq + Hash>() -> TSet<V> {
    TSet::with_hasher(THasher::default())
}

pub fn new_set_wc<V: Eq + Hash>(capacity: usize) -> TSet<V> {
    TSet::with_capacity_and_hasher(capacity, THasher::default())
}

// --- Concurrent Collections ---
pub type TDashMap<K, V> = DashMap<K, V, THasher>;

pub fn new_dash_map<K: Eq + Hash, V>() -> TDashMap<K, V> {
    TDashMap::with_hasher(THasher::default())
}

pub fn new_dash_map_wc<K: Eq + Hash, V>(capacity: usize) -> TDashMap<K, V> {
    TDashMap::with_capacity_and_hasher(capacity, THasher::default())
}
