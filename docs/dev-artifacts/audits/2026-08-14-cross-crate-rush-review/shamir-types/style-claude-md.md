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
