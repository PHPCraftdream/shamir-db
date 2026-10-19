use dashmap::DashMap;
use std::cmp::Eq;
use std::hash::Hash;

pub use shamir_collections::{new_map, new_map_wc, new_set, new_set_wc, THasher, TMap, TSet};

// --- Concurrent Collections (unchanged) ---
pub type TDashMap<K, V> = DashMap<K, V, THasher>;

pub fn new_dash_map<K: Eq + Hash, V>() -> TDashMap<K, V> {
    DashMap::with_hasher(THasher::default())
}

pub fn new_dash_map_wc<K: Eq + Hash, V>(capacity: usize) -> TDashMap<K, V> {
    DashMap::with_capacity_and_hasher(capacity, THasher::default())
}
