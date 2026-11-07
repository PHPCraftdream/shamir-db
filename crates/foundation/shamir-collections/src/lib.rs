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

// Sanctioned (L4-RV-COLLECTIONS-201D84D3-FACET): this crate IS the
// workspace's collection facade — it deliberately defines the `TMap`/`TSet`
// aliases over `IndexMap`/`IndexSet` and exposes std-compatible shims, so
// the `disallowed_types` lint is disabled here at the definition site.
// Every consumer crate keeps the lint and is routed through these aliases.
#![allow(clippy::disallowed_types)]

use indexmap::{IndexMap, IndexSet};
use rustc_hash::FxHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash};

/// Workspace-wide hasher: unseeded `FxHasher` (`BuildHasherDefault`).
///
/// # HashDoS threat model (binding ruling RU8 / @oh)
///
/// `THasher` is deterministic and unseeded, so an adversary who can feed
/// string keys into an Fx-backed collection could in principle precompute a
/// colliding UTF-8 key family. The peer-keyed surfaces that accept
/// client-controlled string keys today:
///
/// - client aliases in `BatchRequest.queries` (`TMap<String, QueryEntry>`)
///   and `BatchRequest.interner_epochs` (`TMap<String, u64>`), including the
///   same maps inside nested `SubBatchOp.batch` payloads;
/// - the `return_only` alias list;
/// - layered-interner overlay keys;
/// - `QueryValue::Map` — the Fx `TMap` inside the core value type itself,
///   which is why seeding individual maps does not hang together (RU8).
///
/// Bounds that keep the attack impractical today: wire frame size limits,
/// the session rate limit, the post-auth rate limit, the 1000-entry
/// decode-time cap on alias-keyed request collections (shamir-query-types
/// `decode_caps`), and the per-repo interner quota
/// (`interner_quota_exceeded`).
///
/// Reopen trigger (RU8, final): a DEMONSTRATED colliding UTF-8 key family
/// within those bounds means swapping `THasher` globally (one type alias),
/// with the impact measured using the existing bench harness. Do NOT
/// re-litigate by seeding individual maps.
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

/// Create an empty insertion-ordered map using [`THasher`].
pub fn new_map<K: Eq + Hash, V>() -> TMap<K, V> {
    IndexMap::with_hasher(THasher::default())
}

/// Create an insertion-ordered map with the requested initial capacity (`wc`).
/// This is an infallible reservation: it panics on capacity overflow and
/// aborts the process on allocation failure. Use [`try_new_map_wc`] when the capacity
/// comes from untrusted input.
pub fn new_map_wc<K: Eq + Hash, V>(capacity: usize) -> TMap<K, V> {
    IndexMap::with_capacity_and_hasher(capacity, THasher::default())
}

/// Try to reserve an insertion-ordered map's initial capacity. Returns
/// `indexmap::TryReserveError` on capacity overflow or allocator failure.
pub fn try_new_map_wc<K: Eq + Hash, V>(
    capacity: usize,
) -> Result<TMap<K, V>, indexmap::TryReserveError> {
    let mut map = new_map();
    map.try_reserve(capacity)?;
    Ok(map)
}

/// Create an empty insertion-ordered set using [`THasher`].
pub fn new_set<V: Eq + Hash>() -> TSet<V> {
    IndexSet::with_hasher(THasher::default())
}

/// Create an insertion-ordered set with the requested initial capacity (`wc`).
/// This is an infallible reservation: it panics on capacity overflow and
/// aborts the process on allocation failure. Use [`try_new_set_wc`] when the capacity
/// comes from untrusted input.
pub fn new_set_wc<V: Eq + Hash>(capacity: usize) -> TSet<V> {
    IndexSet::with_capacity_and_hasher(capacity, THasher::default())
}

/// Try to reserve an insertion-ordered set's initial capacity. Returns
/// `indexmap::TryReserveError` on capacity overflow or allocator failure.
pub fn try_new_set_wc<V: Eq + Hash>(capacity: usize) -> Result<TSet<V>, indexmap::TryReserveError> {
    let mut set = new_set();
    set.try_reserve(capacity)?;
    Ok(set)
}

/// Order-agnostic map using `FxHasher`; see [`THasher`] for the threat model.
pub type TFxMap<K, V> = HashMap<K, V, THasher>;

/// Order-agnostic set using `FxHasher`; see [`THasher`] for the threat model.
pub type TFxSet<T> = HashSet<T, THasher>;

/// Create an empty order-agnostic map using [`THasher`].
pub fn new_fx_map<K: Eq + Hash, V>() -> TFxMap<K, V> {
    HashMap::with_hasher(THasher::default())
}

/// Create an order-agnostic map with the requested initial capacity (`wc`).
/// This is an infallible reservation: it panics on capacity overflow and
/// aborts the process on allocation failure. Use [`try_new_fx_map_wc`] when the capacity
/// comes from untrusted input.
pub fn new_fx_map_wc<K: Eq + Hash, V>(capacity: usize) -> TFxMap<K, V> {
    HashMap::with_capacity_and_hasher(capacity, THasher::default())
}

/// Try to reserve capacity for an order-agnostic map. Returns
/// `std::collections::TryReserveError` on capacity overflow or allocator failure.
pub fn try_new_fx_map_wc<K: Eq + Hash, V>(
    capacity: usize,
) -> Result<TFxMap<K, V>, std::collections::TryReserveError> {
    let mut map = new_fx_map();
    map.try_reserve(capacity)?;
    Ok(map)
}

/// Create an empty order-agnostic set using [`THasher`].
pub fn new_fx_set<T: Eq + Hash>() -> TFxSet<T> {
    HashSet::with_hasher(THasher::default())
}

/// Create an order-agnostic set with the requested initial capacity (`wc`).
/// This is an infallible reservation: it panics on capacity overflow and
/// aborts the process on allocation failure. Use [`try_new_fx_set_wc`] when the capacity
/// comes from untrusted input.
pub fn new_fx_set_wc<T: Eq + Hash>(capacity: usize) -> TFxSet<T> {
    HashSet::with_capacity_and_hasher(capacity, THasher::default())
}

/// Try to reserve capacity for an order-agnostic set. Returns
/// `std::collections::TryReserveError` on capacity overflow or allocator failure.
pub fn try_new_fx_set_wc<T: Eq + Hash>(
    capacity: usize,
) -> Result<TFxSet<T>, std::collections::TryReserveError> {
    let mut set = new_fx_set();
    set.try_reserve(capacity)?;
    Ok(set)
}

#[cfg(test)]
mod tests;
