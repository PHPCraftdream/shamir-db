//! Insertion-ordered collection aliases shared across ShamirDB crates.
//!
//! `TMap`/`TSet` are `IndexMap`/`IndexSet` keyed by a fast `FxHasher`,
//! giving deterministic insertion-order iteration. This crate is a
//! dependency-light leaf (`indexmap` + `rustc-hash` only) so guest-facing
//! crates (query DTOs, builder) can reuse the exact same types as the
//! host without pulling the heavy `shamir-types` graph.
//!
//! Contract notes: removal is asymmetric (`swap_remove` is O(1) but reorders;
//! `shift_remove` preserves order but is O(N)); serde duplicate keys keep the
//! first position with the last value winning (see `TMap` docs).

#![allow(clippy::disallowed_types)]

use indexmap::{IndexMap, IndexSet};
use rustc_hash::FxHasher;
use std::cmp::Eq;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash};

pub type THasher = BuildHasherDefault<FxHasher>;

/// Ordered map that maintains insertion order for predictable iteration.
///
/// # Removal asymmetry
///
/// - `swap_remove` is O(1) but moves the last element into the vacated slot
///   (relative order of the remaining entries is not preserved).
/// - `shift_remove` preserves the relative order of the remaining entries but
///   is O(N): every later entry shifts. Repeated `shift_remove` over N keys is
///   O(N^2) — on hot paths prefer `swap_remove` when survivor order does not
///   matter, or batch/build the map anew.
///
/// # Serde wire contract
///
/// `Deserialize` on input with duplicate keys keeps the FIRST insertion
/// position but the LAST value wins (IndexMap `insert` semantics);
/// serialization emits entries in insertion order. TMap-backed protocol
/// fields therefore have a deterministic wire contract: duplicate key ⇒
/// last value, first position.
pub type TMap<K, V> = IndexMap<K, V, THasher>;

/// Ordered set that maintains insertion order for predictable iteration.
///
/// Removal is asymmetric: `swap_remove` is O(1) but moves the last element
/// into the vacated slot (order not preserved); `shift_remove` preserves the
/// relative order of the remaining entries but is O(N). Repeated
/// `shift_remove` over N elements is O(N^2) — prefer `swap_remove` on hot
/// paths when survivor order does not matter.
///
/// Serde: serialization emits elements in insertion order; duplicates in
/// deserialization input are ignored after the first occurrence.
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

#[cfg(test)]
mod tests;
