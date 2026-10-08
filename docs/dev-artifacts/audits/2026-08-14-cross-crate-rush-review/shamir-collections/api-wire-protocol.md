<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-collections — api-wire-protocol independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The collection surface is complete and serde delegates to pinned IndexMap behavior. Its public documentation is sparse; several other observations are optional API preferences rather than functional defects.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 3 | 0 | 0 | 0 | 0 | 3 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — No documented serialization/wire contract for TMap-backed protocol fields

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

No leaf serde/duplicate contract exists. A decoded map containing alias a twice retains one position and the later operation; the planner cannot recover multiplicity. Reordering independent aliases changes stage tie order, not enforced dependency edges. A discriminating duplicate oracle must consume raw duplicate-bearing input before coalescing; order tests must compare key sequences. Pinned source: https://docs.rs/crate/indexmap/2.14.0/source/src/serde.rs.

Evidence: [crates/shamir-collections/src/lib.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L1); [crates/shamir-collections/Cargo.toml:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/Cargo.toml#L10); [crates/shamir-query-types/src/batch/batch_request.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_request.rs#L87); [crates/shamir-query-types/src/batch/sub_batch_op.rs:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/sub_batch_op.rs#L15); [crates/shamir-query-types/src/batch/planner.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L164); [crates/shamir-query-types/src/batch/planner.rs:841](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L841).

<a id="review-2"></a>

### Claim 2 — Public API mostly undocumented; `_wc` naming cryptic; doctests disabled

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

THasher and eight constructors still lack rustdoc. Naming readability is subjective; disabled doctests follow documented project policy, rather than constituting a separate defect. Public API documentation remains actionable.

Evidence: [crates/shamir-collections/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/Cargo.toml:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/Cargo.toml#L16); [crates/shamir-types/Cargo.toml:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/Cargo.toml#L75); [crates/shamir-query-types/Cargo.toml:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/Cargo.toml#L52).

<a id="review-3"></a>

### Claim 3 — Constructor surface is partially redundant and inconsistently adopted

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Empty helpers overlap Default and callers use multiple valid idioms. No contract requires one canonical spelling. Capacity helpers are useful: IndexMap 2.14.0 exposes with_capacity only on its default-hasher specialization, not TMap. This is optional ergonomics, not a runtime defect. Source: https://docs.rs/crate/indexmap/2.14.0/source/src/map.rs. Parent status consistency: this describes an explicitly sanctioned or optional design/maintenance choice without a violated current contract, so it is not a mandatory defect.

Evidence: [crates/shamir-collections/src/lib.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/src/lib.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L30); [crates/shamir-query-builder/src/batch/batch.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L56); [crates/shamir-types/src/record_view/lens.rs:1064](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L1064); [crates/shamir-engine/src/query/read/aggregate.rs:925](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/aggregate.rs#L925).

<a id="review-4"></a>

### Claim 4 — Half the API missing from the shared façade re-export

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

common.rs re-exports seven ordered-family items and omits six Fx-family items. Collections already exports all thirteen, and direct re-exports cannot independently change their hasher. Completing or retiring the façade is an optional shamir-types API decision. Parent status consistency: this describes an explicitly sanctioned or optional design/maintenance choice without a violated current contract, so it is not a mandatory defect.

Evidence: [crates/shamir-types/src/types/common.rs:5](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/common.rs#L5); [crates/shamir-types/src/record_view/lens.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L33); [crates/shamir-collections/src/lib.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L43); [crates/shamir-collections/src/lib.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L61).

<a id="review-5"></a>

### Claim 5 — Crate-wide `#![allow(clippy::disallowed_types)]` without justification comment

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The local explanation remains absent, while clippy.toml provides an explicit sanction. Narrowing suppression is optional containment; no present forbidden-builder use was found. Parent status consistency: this describes an explicitly sanctioned or optional design/maintenance choice without a violated current contract, so it is not a mandatory defect.

Evidence: [crates/shamir-collections/src/lib.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L9); [clippy.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/clippy.toml#L39).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — Zero in-crate tests, including no serde/ordering pinning test

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

No local test target or module exists. Registered downstream serde roundtrips assert equality and field presence, not explicit entry order or builder identity; they do not establish duplicate rejection.

Evidence: [crates/shamir-collections/src/lib.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L63); [crates/shamir-query-types/src/batch/mod.rs:46](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/mod.rs#L46); [crates/shamir-query-types/src/batch/tests/mod.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/tests/mod.rs#L3); [crates/shamir-query-types/src/batch/tests/batch_types_tests.rs:687](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/tests/batch_types_tests.rs#L687).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

## Evidence and recipe corrections

- The documented project-wide doctest policy is positive counter-evidence to an unexplained-disablement allegation. Keep behavioral examples in registered unit/integration tests unless that policy changes.
- A Vec-of-pairs DTO changes the wire shape and typed API. Do not prescribe it as a documentation-only fix.
- Duplicate rejection must precede IndexMap coalescing. Documenting last-wins is different from guaranteeing unique input aliases.
- TSet serializes as a sequence and deduplicates elements, not as a last-value-wins map.
- The aggregate example already uses new_map_wc; only its type annotation bypasses TMap.
- Canonical construction and complete façade imports are preferences absent a violated contract; their severity is reduced to nit.
- Query-building compliance remains supported: this leaf constructs no queries.
- Parent acceptance classifies the purely optional constructor/façade/lint-containment proposals as N/A, consistently with other modules. Their source facts are retained; no source fix is claimed.

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
