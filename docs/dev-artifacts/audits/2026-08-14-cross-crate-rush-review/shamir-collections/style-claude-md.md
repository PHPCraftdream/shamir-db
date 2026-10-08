<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-collections — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The single closely coupled implementation file and header imports conform to the structural rules. Remaining documentation/import observations are non-runtime debt.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 3 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — No tests anywhere in the pillar-4 anchor crate

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The local suite remains absent, but absence is not a violation of where existing tests must be organized. Registered consumer tests preclude a blanket no-in-repository-coverage assurance.

Evidence: [crates/shamir-collections/src/lib.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L63); [crates/shamir-types/src/types/tests/mod.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/tests/mod.rs#L10); [crates/shamir-collections/Cargo.toml:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/Cargo.toml#L16).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Redundant prelude import `std::cmp::Eq`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The import is unnecessary but remains legal. Neither current nor prospective toolchain warnings were demonstrated.

Evidence: [crates/shamir-collections/src/lib.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L13).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — Doc/comment coverage inconsistent within lib.rs

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

THasher/eight constructors lack rustdoc and the local lint suppression lacks an explanation. External sanction and doctest rationale exist; this is discoverability debt.

Evidence: [crates/shamir-collections/src/lib.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L9); [crates/shamir-collections/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L25); [clippy.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/clippy.toml#L39).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

## Evidence and recipe corrections

- Splitting thirteen closely coupled exports into sibling files is not required by the primary-export rule.
- No inline test module or mod.rs structural violation exists locally.
- The allow-comment facet also duplicates security-crypto.md#2; it is not a separate runtime defect.
- Doctest enablement should respect the documented project-wide policy.
- Do not infer current gate success or future warning behavior from the pinned toolchain metadata.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-collections -- Style & CLAUDE.md structural conformance

## Summary

shamir-collections is a two-file leaf crate (`Cargo.toml` + `src/lib.rs`, 64 lines): five public type aliases (`THasher`, `TMap`, `TSet`, `TFxMap`, `TFxSet`) plus eight constructor functions. Against this review's lens it is essentially conformant: no `mod.rs` files exist (rule vacuously satisfied), all `use` statements are at the file top, there are no inline `#[cfg(test)] mod tests` blocks, the single `lib.rs` legitimately qualifies as one closely-coupled group under the "one file = one primary export" exception, and the crate-level `#![allow(clippy::disallowed_types)]` is explicitly named by `clippy.toml` as "the ONE sanctioned allow-site" for the workspace's std-SipHash type ban. The findings below are minor: zero test coverage in the crate that anchors ideology pillar 4, one redundant prelude import, and inconsistent doc-comment coverage on exported items.

## Findings

### 1. No tests anywhere in the pillar-4 anchor crate

- File: `crates/shamir-collections/src/lib.rs:1-63`; `crates/shamir-collections/Cargo.toml:15-16`
- Severity: low
- Issue: The crate ships 13 public items with zero tests of any kind -- no `src/tests/` module, no integration `tests/` dir, no inline unit tests, and `[lib] doctest = false` disables even future doc-example checking. This is not a violation of the test-LAYOUT rules (those govern where tests live once they exist; rule 5 "never embed inline `#[cfg(test)] mod tests`" is satisfied), but the crate that CLAUDE.md pillar 4 routes every hash-keyed structure in 23 crates through has no executable statement of its two core guarantees: Fx hashing (not `RandomState`) and insertion-order iteration shared host/guest. A regression here propagates workspace-wide undetected by this crate's own suite.
- Failure scenario: A refactor replacing `IndexMap::with_hasher(THasher::default())` semantics or aliasing `TMap` to a non-Fx/unordered backing compiles cleanly today; nothing in-repo catches it at the source.
- Suggested fix: Add `src/tests/` wired per the documented layout (`#[cfg(test)] mod tests;` in `lib.rs`, `src/tests/mod.rs` as a re-export manifest only) with topical files, e.g. `hasher_tests.rs` (assert `TMap`/`TSet`/`TFxMap`/`TFxSet` construct via Fx builder; iteration-order determinism across runs) and `ctor_tests.rs` (capacity pre-allocation for `*_wc` variants).

### 2. Redundant prelude import `std::cmp::Eq`

- File: `crates/shamir-collections/src/lib.rs:13`
- Severity: nit
- Issue: `use std::cmp::Eq;` duplicates an item already in the std prelude; the other four grouped imports each pull something genuinely needed. It currently compiles clean through the gate, but redundant-prelude imports can flip into `unused_imports` warnings ("the item `Eq` is imported redundantly") on toolchain upgrade, i.e. latent gate noise. It also slightly misleads readers into thinking `Eq` needed an explicit path like `Hash`/`BuildHasherDefault` do.
- Suggested fix: Delete line 13.

### 3. Doc/comment coverage inconsistent within lib.rs

- File: `crates/shamir-collections/src/lib.rs:9,17,25,29,33,37,49,53,57,61`
- Severity: nit
- Issue: `THasher` -- the flagship export that `clippy.toml`, CLAUDE.md pillar 4, and hundreds of workspace use-sites name directly -- is the only public alias without a doc comment (`TMap`/`TSet`/`TFxMap`/`TFxSet` all have one-liners); likewise all eight `new_*` constructors are undocumented. Separately, the sanctioned `#![allow(clippy::disallowed_types)]` on line 9 carries no local justification comment; `clippy.toml:37-40` documents it as the sole allow-site for the std-hash-type ban, but a reader standing in this file sees an unexplained blanket crate-wide suppression. CLAUDE.md's annotation culture elsewhere ("annotate ... with `#[allow(...)] // <why>`") points at inline justification.
- Failure scenario: Minor discoverability cost only; IDE hover on the most-referenced export shows nothing, and a future maintainer cannot tell from this file whether the allow is load-bearing or removable.
- Suggested fix: One rustdoc line on `THasher` (workspace default hasher per pillar 4, defined ONCE here so every crate shares identical build), short doc lines on the four ctor families mirroring their return types' docs, and a one-liner on line 9 pointing at `clippy.toml`'s disallowed-types section (pillar-4 rationale). Docs-only change, no behavior.

### Judgment call recorded, not filed: single-file structure vs mod.rs/sibling-file rules

`lib.rs` hosts two nominal families -- ordered (`IndexMap`-backed `TMap`/`TSet`) and order-agnostic (`std::HashMap/HashSet`-backed `TFxMap`/`TFxSet`) plus the shared `THasher`. A strict split into sibling files would contradict both the 64-line reality of the leaf and the counter-rule "No new files unless the task genuinely needs them." The "closely-coupled group" wording covers aliases+ctors over one hasher family; filing no finding deliberately. Note also the consumer-side wrinkle visible in grep (outside this crate's scope): several crates reach these helpers via `shamir_types::types::common::*` re-export paths while others import `shamir_collections::{...}` directly -- if canonical-path uniformity ever matters, that belongs to a shamir-types/api review, not here.

</details>
