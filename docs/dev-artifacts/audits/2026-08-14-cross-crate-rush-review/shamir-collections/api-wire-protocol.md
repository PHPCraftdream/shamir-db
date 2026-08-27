# shamir-collections -- API & wire-protocol design

## Summary

Compliant on query-construction: this crate contains no JSON assembly, no
hand-built filters, and no builder-bypass surface at all — it is a pure type/
constructor leaf. The main theme-relevant gap sits at the seam with its most
important consumers: the exported aliases (`TMap` in particular) are used as
field types of `#[derive(Serialize, Deserialize)]` wire DTOs in
`shamir-query-types` (`BatchRequest.queries/bind/interner_epochs`,
`wire/db_message.rs`), yet this crate documents no serialization contract —
entry-order dependence across formats, duplicate-key coalescing on decode,
and canonicalization expectations are all unspecified. Public-interface
quality otherwise needs only modest polish (undocumented exports, cryptic
`_wc` names, fragmented construction idioms); there are no functional defects.

## Findings

### 1. No documented serialization/wire contract for TMap-backed protocol fields
**File:** `crates/shamir-collections/src/lib.rs:20` (+ `Cargo.toml:10`)
**Severity:** medium

**Issue:** `Cargo.toml` enables indexmap's `serde` feature, and the consumer
grep shows the result lands directly on the wire:
`shamir-query-types/src/batch/batch.rs:33,38` fields `queries:
TMap<String, QueryEntry>` / `interner_epochs: TMap<String, u64>`, and the
derived structs in `shamir-query-types/src/wire/db_message.rs:29,280`
(`Serialize, Deserialize`) carry these maps into every client/server message.
This crate — which owns the abstraction — says nothing about what that means
on the wire: whether insertion order is part of the contract, that order
survives round-trip *only* through order-preserving formats/serializers, or
that duplicate keys in an untrusted payload silently coalesce (last value
wins, first position retained) instead of being rejected.

**Failure scenario:** a non-Rust guest/WASM host or proxy that round-trips
requests through an unordered map representation (or canonicalizing JSON /
MessagePack tooling) reorders entries; alias-keyed batch semantics whose
execution depends on insertion sequence change silently, and two ops sharing
an alias in one decoded request merge instead of erroring. "Checksums
everywhere" (goal 4) cannot be extended over such payloads because byte-level
canonical form is undefined for these fields.

**Suggested fix:** add a crate-level doc section defining the wire semantics
of `TMap`/`TSet` when serialized: insertion order is carried by
order-preserving formats only; duplicate-key behavior is last-wins and MUST
be validated upstream; no cross-language canonical form. If alias uniqueness/
order carries semantic weight in `BatchRequest`, recommend (in this doc)
using `Vec<(K, V)>` pairs for those specific DTO fields rather than a hash map.

### 2. Public API mostly undocumented; `_wc` naming cryptic; doctests disabled
**File:** `crates/shamir-collections/src/lib.rs:17-63` (+ `Cargo.toml:16`)
**Severity:** low

**Issue:** `THasher` — the single most-relied-upon export (workspace pillar 4;
imported by `shamir-tx`, `shamir-engine`, `shamir-index`, `shamir-server`,
`shamir-db`) — has zero rustdoc; the DOS-protection-vs-speed rationale lives
only in CLAUDE.md. The eight constructor functions have no doc comments, and
names like `new_map_wc` require guessing ("with capacity"). `[lib] doctest =
false` guarantees even future examples would not compile-checked. For a crate
whose entire product is its public interface, bare signatures are thin
documentation.

**Failure scenario:** none functional; discoverability/misuse cost (e.g. a
contributor reaching for `std::collections::HashMap::new()` habits instead of
the blessed constructors).

**Suggested fix:** add `///` docs to `THasher` (rationale + pointer to pillar
4), each constructor, and rename `_wc` → `with_capacity` suffix spelling at
the next natural breaking window; re-enable doctests or state why they stay
off.

### 3. Constructor surface is partially redundant and inconsistently adopted
**File:** `crates/shamir-collections/src/lib.rs:25-63`
**Severity:** low

**Issue:** All eight free functions duplicate paths already available
directly on the exported types: because `THasher = BuildHasherDefault<FxHasher>`
satisfies `BuildHasher + Default`, `TMap::<K,V>::default()`,
`TMap::with_capacity(n)`, `TFxSet::<T>::default()` etc. work identically — so
the ctors add ergonomics only, not safety (the aliases already pin the hasher).
Workspace usage shows three coexisting idioms for identical construction:
`new_map()` (`shamir-query-builder/src/batch/batch.rs:56`),
`TMap::default()` (`shamir-query-types` planner tests; engine test
`p1059_online_create_index_tests.rs:117`), fully spelled
`indexmap::IndexMap<String, QueryValue, shamir_collections::THasher>` ignoring
the `TMap` alias entirely (`shamir-engine/src/query/read/aggregate.rs:925`),
and `TFxMap::with_capacity_and_hasher(n, THasher::default())`
(`shamir-types/src/record_view/lens.rs:1064`).

**Failure scenario:** none at runtime; API-discoverability fragmentation makes
the ctor set look authoritative while real code bypasses it (and vice versa),
and future edits have no single idiom to conform to.

**Suggested fix:** declare one canonical idiom in the crate-level doc. Either
keep the ctors as the blessed form (then fix the `aggregate.rs` /
`lens.rs`-style call sites to use them and document that `Default`/
`with_capacity` are equivalent fallbacks) or drop the duplicate fns in favor
of `.default()`/`.with_capacity()`. The right moment is whenever this crate's
API next changes anyway.

### 4. Half the API missing from the shared façade re-export
**File:** `crates/shamir-collections/src/lib.rs:43-62` vs
`crates/shamir-types/src/types/common.rs:5`
**Severity:** low (shared blame with `shamir-types`, root cause here)

**Issue:** `shamir-types::types::common` presents itself as the façade but
re-exports only `{new_map, new_map_wc, new_set, new_set_wc, TMap, TSet,
THasher}` — omitting `TFxMap`, `TFxSet`, and their four constructors. Files
therefore need twin imports in one header, e.g.
`crates/shamir-types/src/record_view/lens.rs:33-34` (`crate::types::common::
THasher` **and** `shamir_collections::TFxMap`) and
`codecs/interned/messagepack.rs:14+22`.

**Failure scenario:** none; perpetual import friction and inconsistent
lint/config drift risk if the two sources ever diverge (e.g. hasher swap done
in one path).

**Suggested fix:** make `common.rs` re-export the full set (all 12 items) or
stop maintaining the partial façade and standardize on direct
`shamir_collections::*` imports; this crate should expose all twelve as one
coherent group so neither split is load-bearing.

### 5. Crate-wide `#![allow(clippy::disallowed_types)]` without justification comment
**File:** `crates/shamir-collections/src/lib.rs:9`
**Severity:** nit

**Issue:** The blanket attr is needed (defining the `std::collections`
aliases here is exactly the sanctioned exception), but repo culture — CLAUDE.md
contention-model comments, `// O(N) ack:` pattern — expects an inline *why*.
A blanket allow also mutes the RandomState ban for anything later added to
this file (a stray helper struct with `HashMap<String, _>` defaults would
compile silently).

**Suggested fix:** replace with an attributed comment stating the exception
("aliases over std::collections are this crate's purpose") and/or scope the
allow to the alias definitions plus fx fns.

### 6. Zero in-crate tests, including no serde/ordering pinning test
**File:** `crates/shamir-collections/` (no `tests/` directory at all)
**Severity:** low

**Issue:** Despite advertising serde (`features = ["serde"]` on indexmap) and
being consumed as wire DTO field types (finding 1), nothing pins the
properties the workspace leans on: Fx-hasher wiring of each constructor,
insertion-order iteration across insert/remove/reintroduce, and a
`TMap`→JSON/msgpack→`TMap` round-trip preserving entry order. Coverage today
exists only incidentally downstream. Even three small files under
`src/tests/` per the repo layout would lock the contract the other findings
say should be documented.

**Suggested fix:** add `tests/hash_wiring_tests.rs` (constructed maps' hasher
is Fx — observable via deterministic iteration of equal-priority keys), and
`tests/serde_roundtrip_tests.rs` asserting insertion-order preservation and
documented duplicate-key behavior. Mark as nits the separate items: remove
redundant `use std::cmp::Eq;` (`lib.rs:13`, prelude item).
