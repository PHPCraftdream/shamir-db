//! Insertion-ordered collection aliases shared across ShamirDB crates.
//!
//! `TMap`/`TSet` are `IndexMap`/`IndexSet` keyed by a fast `FxHasher`,
//! giving deterministic insertion-order iteration. This crate is a
//! dependency-light leaf (`indexmap` + `rustc-hash` only) so guest-facing
//! crates (query DTOs, builder) can reuse the exact same types as the
//! host without pulling the heavy `shamir-types` graph.

#![allow(clippy::disallowed_types)]

use rustc_hash::FxHasher;
use indexmap::{IndexMap, IndexSet};
use std::cmp::Eq;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash};

pub type THasher = BuildHasherDefault<FxHasher>;

/// Ordered map that maintains insertion order for predictable iteration
pub type TMap<K, V> = IndexMap<K, V, THasher>;

/// Ordered set that maintains insertion order for predictable iteration
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

/// Order-agnostic map: `std::HashMap` with `FxHasher`. ~15-20% faster than
/// `TMap` for hot-path lookups that do not need insertion-order iteration.
pub type TFxMap<K, V> = HashMap<K, V, THasher>;

/// Order-agnostic set: `std::HashSet` with `FxHasher`. ~15-20% faster than
/// `TSet` for hot-path membership tests that do not need insertion order.
pub type TFxSet<T> = HashSet<T, THasher>;

pub fn new_fx_map<K: Eq + Hash, V>() -> TFxMap<K, V> {
    HashMap::with_hasher(THasher::default())
}

pub fn new_fx_map_wc<K: Eq + Hash, V>(capacity: usize) -> TFxMap<K, V> {
    HashMap::with_capacity_and_hasher(capacity, THasher::default())
}

pub fn new_fx_set<T: Eq + Hash>() -> TFxSet<T> {
    HashSet::with_hasher(THasher::default())
}

pub fn new_fx_set_wc<T: Eq + Hash>(capacity: usize) -> TFxSet<T> {
    HashSet::with_capacity_and_hasher(capacity, THasher::default())
}
