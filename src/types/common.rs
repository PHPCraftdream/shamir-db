use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;
use fxhash::FxHasher;

pub type THasher = BuildHasherDefault<FxHasher>;

pub type TSet<T> = HashSet<T, THasher>;

pub type TMap<K, V> = HashMap<K, V, THasher>;

pub fn new_map<K, V>() -> TMap<K, V> {
    TMap::with_hasher(THasher::default())
}

pub fn new_set<V>() -> TSet<V> {
    TSet::with_hasher(THasher::default())
}