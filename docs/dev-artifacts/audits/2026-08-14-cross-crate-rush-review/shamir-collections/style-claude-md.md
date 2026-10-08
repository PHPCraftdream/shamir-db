<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-collections — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The leaf remains structurally conformant and all three minor observations remain. Test absence is coverage debt, not a test-layout violation; import/doc nits are not runtime defects.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 3 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — No tests anywhere in the pillar-4 anchor crate

Status: `confirmed-open`. Current risk: `low`.

No test files or registration exist and doctests are disabled. The absence does not violate rules governing the layout of tests that already exist.

Evidence: [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16); [CLAUDE.md:573](../../../../../CLAUDE.md#L573).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Redundant prelude import `std::cmp::Eq`

Status: `confirmed-open`. Current risk: `nit`.

The redundant Eq import remains. A future toolchain warning is speculative and was not established by source or execution.

Evidence: [crates/shamir-collections/src/lib.rs:13](../../../../../crates/shamir-collections/src/lib.rs#L13); [rust-toolchain.toml:15](../../../../../rust-toolchain.toml#L15).

Grouping/duplicate: `correctness-tdd.md#3`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Doc/comment coverage inconsistent within lib.rs

Status: `confirmed-open`. Current risk: `nit`.

THasher and all constructors remain undocumented, and the broad allow still has no local explanation. Both facets remain; clippy.toml supplies the external sanction.

Evidence: [crates/shamir-collections/src/lib.rs:9](../../../../../crates/shamir-collections/src/lib.rs#L9); [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:25](../../../../../crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/src/lib.rs:61](../../../../../crates/shamir-collections/src/lib.rs#L61); [clippy.toml:39](../../../../../clippy.toml#L39).

Grouping/duplicate: `api-wire-protocol.md#2`. This row is not another independent defect.

## Corrections and qualified non-findings

- The single closely-coupled aliases/constructors file still fits CLAUDE.md:505; a sibling-file split is not required.
- All imports remain at the header; there are no inline test modules or mod.rs layout violations in this crate.
- The crate-wide allow remains intentionally sanctioned. Missing local justification and future lint containment are separate from a present prohibited-type defect.
- Hasher type assertions and explicit ordered-key assertions are appropriate tests; identical iteration does not itself establish Fx hashing.
- Downstream tests exist and are wired, so nothing in-repo catches it is too absolute. Existing equality/shared-reference oracles nevertheless do not replace the missing direct contract guards.
- Do not present speculative toolchain-upgrade gate noise as demonstrated behavior.
- The doc facet duplicates api-wire-protocol finding 2; the allow-comment facet separately duplicates security-crypto finding 2.

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
