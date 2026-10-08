<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-collections — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

All six interface observations remain materially present. Several explanations and proposed tests need correction: custom-hasher aliases cannot use TMap::with_capacity as claimed, order-sensitive tests need explicit iteration assertions, and duplicate rejection must precede coalescing.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 6 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — No documented serialization/wire contract for TMap-backed protocol fields

Status: `confirmed-open`. Current risk: `medium`.

The leaf still provides no serde/order/duplicate contract. BatchRequest queries and interner_epochs and SubBatchOp bind remain derived TMap fields. Pinned IndexMap's visitor inserts entries and therefore retains the first position while replacing duplicate values; planning uses decoded alias order.

Evidence: [crates/shamir-collections/src/lib.rs:1](../../../../../crates/shamir-collections/src/lib.rs#L1); [crates/shamir-collections/Cargo.toml:10](../../../../../crates/shamir-collections/Cargo.toml#L10); [crates/shamir-query-types/src/batch/batch_request.rs:40](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L40); [crates/shamir-query-types/src/batch/batch_request.rs:87](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L87); [crates/shamir-query-types/src/batch/batch_request.rs:109](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L109); [crates/shamir-query-types/src/batch/sub_batch_op.rs:15](../../../../../crates/shamir-query-types/src/batch/sub_batch_op.rs#L15); [crates/shamir-query-types/src/batch/planner.rs:164](../../../../../crates/shamir-query-types/src/batch/planner.rs#L164); [crates/shamir-query-types/src/batch/planner.rs:841](../../../../../crates/shamir-query-types/src/batch/planner.rs#L841).

<a id="review-2"></a>

### Claim 2 — Public API mostly undocumented; `_wc` naming cryptic; doctests disabled

Status: `confirmed-open`. Current risk: `low`.

THasher and all eight constructor functions still lack rustdoc; the _wc names and disabled doctests remain. This is discoverability debt without a demonstrated functional failure.

Evidence: [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:25](../../../../../crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/src/lib.rs:61](../../../../../crates/shamir-collections/src/lib.rs#L61); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16).

<a id="review-3"></a>

### Claim 3 — Constructor surface is partially redundant and inconsistently adopted

Status: `confirmed-open`. Current risk: `low`.

Free constructors, Default and explicit with_capacity_and_hasher construction coexist without a declared canonical idiom. Empty constructors overlap Default, but the alleged TMap::with_capacity equivalence is false: pinned IndexMap defines that method only for its default-hasher specialization.

Evidence: [crates/shamir-collections/src/lib.rs:25](../../../../../crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/src/lib.rs:30](../../../../../crates/shamir-collections/src/lib.rs#L30); [Cargo.lock:1783](../../../../../Cargo.lock#L1783); [crates/shamir-query-builder/src/batch/batch.rs:56](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L56); [crates/shamir-query-types/src/batch/tests/planner_tests.rs:53](../../../../../crates/shamir-query-types/src/batch/tests/planner_tests.rs#L53); [crates/shamir-types/src/record_view/lens.rs:1064](../../../../../crates/shamir-types/src/record_view/lens.rs#L1064); [crates/shamir-engine/src/query/read/aggregate.rs:925](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L925).

<a id="review-4"></a>

### Claim 4 — Half the API missing from the shared façade re-export

Status: `confirmed-open`. Current risk: `low`.

common.rs still re-exports seven ordered-family items and omits six Fx-family items, producing split imports. The collections crate already exports all thirteen items; ownership of any façade change is shamir-types, not a missing collections export.

Evidence: [crates/shamir-types/src/types/common.rs:5](../../../../../crates/shamir-types/src/types/common.rs#L5); [crates/shamir-types/src/record_view/lens.rs:33](../../../../../crates/shamir-types/src/record_view/lens.rs#L33); [crates/shamir-types/src/record_view/lens.rs:34](../../../../../crates/shamir-types/src/record_view/lens.rs#L34); [crates/shamir-collections/src/lib.rs:43](../../../../../crates/shamir-collections/src/lib.rs#L43); [crates/shamir-collections/src/lib.rs:61](../../../../../crates/shamir-collections/src/lib.rs#L61).

<a id="review-5"></a>

### Claim 5 — Crate-wide `#![allow(clippy::disallowed_types)]` without justification comment

Status: `confirmed-open`. Current risk: `nit`.

The allow still lacks a local justification comment. Its purpose is explicitly documented in clippy.toml, so it is sanctioned rather than a present lint-policy violation.

Evidence: [crates/shamir-collections/src/lib.rs:9](../../../../../crates/shamir-collections/src/lib.rs#L9); [clippy.toml:39](../../../../../clippy.toml#L39).

Grouping/duplicate: `security-crypto.md#2`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — Zero in-crate tests, including no serde/ordering pinning test

Status: `confirmed-open`. Current risk: `low`.

No local tests or registration were added. Reachable downstream round-trip tests exercise maps but generally assert map equality, which cannot detect entry reordering; they do not replace constructor/hasher/removal/duplicate guards.

Evidence: [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16); [crates/shamir-query-types/src/batch/mod.rs:46](../../../../../crates/shamir-query-types/src/batch/mod.rs#L46); [crates/shamir-query-types/src/batch/tests/mod.rs:3](../../../../../crates/shamir-query-types/src/batch/tests/mod.rs#L3); [crates/shamir-query-types/src/batch/tests/batch_types_tests.rs:687](../../../../../crates/shamir-query-types/src/batch/tests/batch_types_tests.rs#L687).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

## Corrections and qualified non-findings

- The old batch/batch.rs citation has moved to batch/batch_request.rs; bind is a SubBatchOp field, not a top-level BatchRequest field.
- TMap serializes as a map; TSet serializes as a sequence. Their duplicate semantics should not be described as identical map last-value-wins behavior.
- Rejecting duplicate aliases upstream after IndexMap decoding is too late: multiplicity has been lost. Rejection requires a validating deserialization visitor or another representation before coalescing.
- Wire reordering changes independent-query tie-breaking; dependency edges remain enforced. Use explicit after/data-flow edges for required execution dependencies.
- Lack of cross-language canonicalization does not prevent checksums over transmitted bytes. AUTH_PROTOCOL.md:904 explicitly leaves map ordering unrestricted and separately defines authentication canonical bytes.
- Vec<(K,V)> would change the protocol representation; it is an option requiring compatibility analysis, not an automatic documentation-only fix.
- Do not recommend replacing _wc constructors with TMap::with_capacity; use with_capacity_and_hasher when constructing the custom-hasher aliases directly.
- The aggregate.rs example already constructs with new_map_wc; it bypasses the alias in its type annotation, not the constructor.
- There are thirteen public items, not twelve. Direct pub use re-exports preserve the same types and cannot independently swap their hasher.
- Deterministic insertion-order iteration is not a hasher-identity oracle. Round-trip order tests must compare iterated keys/entries, not only map equality.
- The separate Eq nit remains open; Hash must stay imported.
- The query-construction non-finding remains valid: this leaf assembles no queries, JSON, filters or wire operations.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
