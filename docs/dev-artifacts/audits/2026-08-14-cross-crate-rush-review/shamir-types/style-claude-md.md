<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-types — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The access mega-file, duplicate manual error type, and local imports remain. These are low/nit maintainability issues. The test-wrapper rule violation is refuted: wrappers are inside test files, and more than one file uses that shape.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 7 | 5 | 0 | 0 | 1 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `access.rs` bundles identity, mode-bits, policy and error types into one 716-line file -- one-file-one-export violation

Status: `confirmed-open`. Current risk: `low`.

The file still groups identity, modes, resource paths, metadata, and policy exports. A split can improve compliance with the one-primary-export convention, but there is no runtime failure or severity-Medium mechanism.

Evidence: [crates/shamir-types/src/access.rs:22](../../../../../crates/shamir-types/src/access.rs#L22); [crates/shamir-types/src/access.rs:121](../../../../../crates/shamir-types/src/access.rs#L121); [crates/shamir-types/src/access.rs:197](../../../../../crates/shamir-types/src/access.rs#L197); [crates/shamir-types/src/access.rs:371](../../../../../crates/shamir-types/src/access.rs#L371); [crates/shamir-types/src/access.rs:702](../../../../../crates/shamir-types/src/access.rs#L702); [CLAUDE.md:542](../../../../../CLAUDE.md#L542).

<a id="review-2"></a>

### Claim 2 — Second public `CodecError` enum with manual impls duplicates the crate's thiserror error type

Status: `confirmed-open`. Current risk: `low`.

Both error enums and the manual bincode Display/Error implementations remain; thiserror policy and API naming debt remain.

Evidence: [crates/shamir-types/src/codecs/basic/bincode.rs:8](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L8); [crates/shamir-types/src/codecs/basic/bincode.rs:14](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L14); [crates/shamir-types/src/codecs/error.rs:4](../../../../../crates/shamir-types/src/codecs/error.rs#L4).

Grouping/duplicate: `error-handling-lifecycle.md:2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Mid-function imports in production code violate imports-at-top

Status: `confirmed-open`. Current risk: `nit`.

Two local Entry imports remain, and SeedableRng remains local to thread_local initialization. Entry imports plainly qualify as header-hoisting cleanup; the macro-contained import is stylistic and should be assessed against the documented macro-body exception, not treated as a runtime defect.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:161](../../../../../crates/shamir-types/src/core/interner/interner.rs#L161); [crates/shamir-types/src/core/interner/interner.rs:349](../../../../../crates/shamir-types/src/core/interner/interner.rs#L349); [crates/shamir-types/src/types/record_id.rs:85](../../../../../crates/shamir-types/src/types/record_id.rs#L85); [CLAUDE.md:657](../../../../../CLAUDE.md#L657).

<a id="review-4"></a>

### Claim 4 — `types/tests/value_tests.rs` retains the legacy inline `#[cfg(test)] mod tests { ... }` wrapper shape

Status: `refuted`. Current risk: —.

The wrapper exists inside a dedicated tests file, whereas the rule forbids embedding such tests inside implementation files. record_id_tests.rs also uses a wrapper, refuting the lone-file claim. Flattening remains optional cosmetic cleanup; deprecation allowance is appropriate for UserValue-focused tests.

Evidence: [crates/shamir-types/src/types/tests/value_tests.rs:1](../../../../../crates/shamir-types/src/types/tests/value_tests.rs#L1); [crates/shamir-types/src/types/tests/record_id_tests.rs:1](../../../../../crates/shamir-types/src/types/tests/record_id_tests.rs#L1); [crates/shamir-types/src/types/tests/mod.rs:8](../../../../../crates/shamir-types/src/types/tests/mod.rs#L8); [CLAUDE.md:621](../../../../../CLAUDE.md#L621).

<a id="review-5"></a>

### Claim 5 — Mid-function imports scattered through test files

Status: `confirmed-open`. Current risk: `nit`.

The cited function-local imports still exist, including duplicated RecordView imports in merge tests. These are import-convention cleanup, not runtime risk.

Evidence: [crates/shamir-types/src/tests/access_tests.rs:265](../../../../../crates/shamir-types/src/tests/access_tests.rs#L265); [crates/shamir-types/src/core/interner/tests/interner_tests.rs:534](../../../../../crates/shamir-types/src/core/interner/tests/interner_tests.rs#L534); [crates/shamir-types/src/core/interner/tests/interner_tests.rs:804](../../../../../crates/shamir-types/src/core/interner/tests/interner_tests.rs#L804); [crates/shamir-types/src/codecs/interned/tests/messagepack_tests.rs:667](../../../../../crates/shamir-types/src/codecs/interned/tests/messagepack_tests.rs#L667); [crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs:438](../../../../../crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs#L438); [crates/shamir-types/src/codecs/interned/tests/merge_storage_bytes_tests.rs:296](../../../../../crates/shamir-types/src/codecs/interned/tests/merge_storage_bytes_tests.rs#L296); [crates/shamir-types/src/codecs/interned/tests/merge_storage_bytes_tests.rs:318](../../../../../crates/shamir-types/src/codecs/interned/tests/merge_storage_bytes_tests.rs#L318); [crates/shamir-types/src/record_view/tests/scalar_ref_cmp_tests.rs:193](../../../../../crates/shamir-types/src/record_view/tests/scalar_ref_cmp_tests.rs#L193); [crates/shamir-types/src/macros/tests/mpack_tests.rs:332](../../../../../crates/shamir-types/src/macros/tests/mpack_tests.rs#L332).

<a id="review-6"></a>

### Claim 6 — Dead "Tests" section banner left behind after inline-test extraction

Status: `confirmed-open`. Current risk: `nit`.

The implementation file still ends with an empty Tests divider; actual tests are registered in core/tests.

Evidence: [crates/shamir-types/src/core/sort_codec.rs:152](../../../../../crates/shamir-types/src/core/sort_codec.rs#L152); [crates/shamir-types/src/core/tests/mod.rs:1](../../../../../crates/shamir-types/src/core/tests/mod.rs#L1).

<a id="review-7"></a>

### Claim 7 — Inconsistent test-manifest visibility across `tests/mod.rs` files

Status: `not-applicable`. Current risk: —.

Mixed pub/private module declarations remain, but the policy's example does not require uniform visibility. Registered private modules run normally under the parent test gate; no functional or structural-registration defect follows.

Evidence: [crates/shamir-types/src/tests/mod.rs:1](../../../../../crates/shamir-types/src/tests/mod.rs#L1); [crates/shamir-types/src/types/tests/mod.rs:8](../../../../../crates/shamir-types/src/types/tests/mod.rs#L8); [crates/shamir-types/src/record_view/tests/mod.rs:9](../../../../../crates/shamir-types/src/record_view/tests/mod.rs#L9); [crates/shamir-types/src/core/interner/mod.rs:14](../../../../../crates/shamir-types/src/core/interner/mod.rs#L14).

## Corrections and qualified non-findings

- Module manifests remain declaration/reexport-only, topic tests are reachable through cfg(test), and no implementation-file inline test body was found.
- The review's fixed count of 21 test-file groups is stale; do not preserve it as a current inventory.
- Style cleanup must remain separate from substantive fixes and must not inherit runtime High/Medium severity automatically.
- The claimed sole nested test wrapper and the claimed ban applying to tests-directory files are both incorrect.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-types -- Style & CLAUDE.md structural conformance

## Summary

The crate is largely conformant on the structural conventions: every `mod.rs` (lib, types, codecs, core, core/interner, record_view, macros, basic, interned) is re-export-only, tests live in per-module `tests/` directories wired through manifest `mod.rs` files with topic-split coverage for all 21 test-file groups, and no inline `#[cfg(test)] mod tests { ... }` survives inside any implementation file. The two real structural outliers are `access.rs`, which concentrates ~15 public exports spanning three loosely-coupled domains in one 716-line file against the explicit one-file-one-export rule, and a second hand-rolled `CodecError` enum in `codecs/basic/bincode.rs` that duplicates the crate's canonical thiserror-based `codec::error::CodecError`. A handful of mid-function `use` statements (3 in production code, ~9 in test files) violate the mandatory imports-at-top rule.

## Findings

### 1. `access.rs` bundles identity, mode-bits, policy and error types into one 716-line file -- one-file-one-export violation
- **File:** `crates/shamir-types/src/access.rs:1-716`
- **Severity:** medium
- **Issue:** CLAUDE.md ("One file = one primary export") permits one struct/enum/trait plus a closely-coupled group. This single file defines 15 public items across three domains: principal/identity projection (`OWNER_SYSTEM`, `principal64`, `principal64_from_username`, `Actor`), POSIX mode-bit math (`Mode`, `MODE_SETUID`, `PermClass`, `Perm`), resource addressing (`ResourcePath`), metadata envelope (`ResourceMeta`) and policy evaluation (`Action`, `AccessError`, `trace_access`, `action_perm`, `class_of`, `permits`). Sibling modules in this same crate (`touch_ind.rs`, `value_error.rs`, `record_view/kind.rs`) demonstrate the intended granularity; `access.rs` is the outlier.
- **Failure scenario:** unrelated access-model edits (e.g. adding an `Action` variant) force re-diffs of a file whose other sections are stable; `git blame` mixes identity-minting changes with policy changes; reviewers cannot tell at a glance which concern a hunk belongs to.
- **Suggested fix:** split into sibling files (`actor.rs`, `principal.rs`, `resource_path.rs`, `action.rs`, `mode.rs`, `resource_meta.rs`, `policy.rs`, `access_error.rs`) under `src/access/` with a re-export-only `mod.rs` that preserves today's `crate::access::*` paths so downstream crates (`shamir-engine`, `shamir-db`) keep compiling unchanged.

### 2. Second public `CodecError` enum with manual impls duplicates the crate's thiserror error type
- **File:** `crates/shamir-types/src/codecs/basic/bincode.rs:7-22` vs `crates/shamir-types/src/codecs/error.rs:3-9`
- **Severity:** medium
- **Issue:** Two distinct public enums named `CodecError` coexist: the crate-canonical `codecs::CodecError` (thiserror-derived, per the documented "`thiserror` for library error enums" rule) and `basic::bincode::CodecError` with by-hand `Display` + `std::error::Error` impls. Both are consumed cross-crate via deep paths (`shamir-engine/src/table/interner_manager.rs:12`, `shamir-engine/src/table/record_counter.rs:20`, `shamir-index` tests).
- **Failure scenario:** a caller importing `shamir_types::codecs::CodecError` alongside `bincode::{from_bytes, to_bytes}` hits a confusing name collision and must alias; the two enums drift apart as error reporting evolves (the bincode variant has no structured fields and bypasses workspace error hygiene).
- **Suggested fix:** converge on one type -- either wrap `bincode::Error` into the canonical `codecs::CodecError` from `bincode::to_bytes/from_bytes`, or move the bincode variant into its own distinctly-named file/type (`BincodeError`) if API stability requires keeping both. Either way, convert to thiserror.

### 3. Mid-function imports in production code violate imports-at-top
- **File:** `crates/shamir-types/src/core/interner/interner.rs:161` and `:349`; `crates/shamir-types/src/types/record_id.rs:85`
- **Severity:** medium
- **Issue:** CLAUDE.md mandates all `use` statements in the file header, with three narrow exceptions none of which apply here: `use dashmap::mapref::entry::Entry;` appears inside `Interner::touch_ind` (line 161) and again inside `Interner::touch_with_id` (line 349), and `use rand::SeedableRng;` sits inside `RecordId::fill_random_tail`'s `thread_local!` initializer (line 85). There is no name-collision comment and no cfg-gating; hoisting compiles identically (no other `Entry` or `SeedableRng` is referenced anywhere else in either file's header).
- **Failure scenario:** readers scanning headers miss trait deps; duplicated local imports (the `Entry` import exists twice) invite divergence; automation that audits header imports reports false negatives.
- **Suggested fix:** hoist `use dashmap::mapref::entry::Entry;` once to `interner.rs`'s header block and delete both locals; hoist `use rand::SeedableRng;` next to `use rand::RngCore;` in `record_id.rs`.

### 4. `types/tests/value_tests.rs` retains the legacy inline `#[cfg(test)] mod tests { ... }` wrapper shape
- **File:** `crates/shamir-types/src/types/tests/value_tests.rs:1-13`
- **Severity:** low
- **Issue:** Every other test file in the crate (~20 of them) uses flat top-level `#[test]` functions; this lone file nests everything inside `#[cfg(test)] #[allow(deprecated)] mod tests { ... }`. That is precisely the shape CLAUDE.md rule 5 bans in implementation files and flags as "such blocks are themselves being migrated to `tests/`" -- the migration landed here but kept the wrapper. It also makes this file inconsistent with its own siblings `base_tests.rs` / `record_id_tests.rs`, and the module-level `#[allow(deprecated)]` silently widens suppression scope over the whole file.
- **Failure scenario:** the file gets copied as a template, propagating the deprecated pattern; the blanket deprecation allow masks genuine new uses of deprecated APIs added later.
- **Suggested fix:** flatten to top-level `#[test]` fns like siblings, narrowing `#[allow(deprecated)]` to only the items exercising `UserValue`.

### 5. Mid-function imports scattered through test files
- **File:** `src/tests/access_tests.rs:265-266`; `src/core/interner/tests/interner_tests.rs:534,804`; `src/codecs/interned/tests/messagepack_tests.rs:667`; `src/codecs/interned/tests/storage_bytes_tests.rs:438`; `src/codecs/interned/tests/merge_storage_bytes_tests.rs:296,318`; `src/record_view/tests/scalar_ref_cmp_tests.rs:193`; `src/macros/tests/mpack_tests.rs:332`
- **Severity:** low
- **Issue:** Same imports-at-top rule, test-side: function-local `use` statements inside individual `#[test]` fns. None fall under the documented exceptions (`use super::*` / collision-with-comment / cfg-gated macro body). `merge_storage_bytes_tests.rs` even repeats the identical `use crate::record_view::RecordView;` in two adjacent functions.
- **Failure scenario:** duplicate imports drift out of sync when one is edited; lower readability of long test bodies.
- **Suggested fix:** hoist each into the owning test file's header.

### 6. Dead "Tests" section banner left behind after inline-test extraction
- **File:** `crates/shamir-types/src/core/sort_codec.rs:152-154`
- **Severity:** nit
- **Issue:** The file ends with a `// Tests` divider banner followed by nothing -- leftover scaffolding from before the tests moved to `core/tests/sort_codec_tests.rs`.
- **Failure scenario:** minor: misleads a reader into expecting content below.
- **Suggested fix:** delete the three-line banner (or replace with a pointer doc-comment to `core/tests/sort_codec_tests.rs`).

### 7. Inconsistent test-manifest visibility across `tests/mod.rs` files
- **File:** e.g. `src/tests/mod.rs`, `src/core/tests/mod.rs`, `src/core/interner/tests/mod.rs`, `src/codecs/basic/tests/mod.rs`, `src/codecs/interned/tests/mod.rs` use `pub mod x_tests;`; `src/types/tests/mod.rs`, `src/record_view/tests/mod.rs`, `src/macros/tests/mod.rs`, `src/types/tests/value_tests.rs` entries use private `mod ...` (some with redundant extra `#[cfg(test)]` on top of the parent's existing gate)
- **Severity:** nit
- **Issue:** CLAUDE.md's example shows uniform `pub mod value_tests;` manifests; the crate mixes `pub mod` / private `mod` / `#[cfg(test)] pub mod` freely between sibling manifests (and `core/interner/mod.rs` wires its tests as `pub mod tests` while every other parent uses private `mod tests`). Purely cosmetic -- visibility differences are unobservable given the `#[cfg(test)]` gate at the parent.
- **Failure scenario:** none functional; a reader comparing modules cannot infer convention.
- **Suggested fix:** pick one form (plain `pub mod x_tests;` matching the doc example) and normalize all manifests in a style-only sweep committed separately, per CLAUDE.md's style-commit rule.

</details>
