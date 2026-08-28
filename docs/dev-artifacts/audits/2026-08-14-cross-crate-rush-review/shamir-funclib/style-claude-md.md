# shamir-funclib -- Style & CLAUDE.md structural conformance

## Summary

The crate is in strong conformance with CLAUDE.md's structural rules: all 13 `mod.rs` files (every one under a `tests/` dir) are re-export-only manifests per the mandated template; there are zero inline `#[cfg(test)] mod tests { ... }` blocks; the bench uses the mandated `bench_scale_tool::Harness`; every module keeps imports at the header except a handful of function-body `use`s; and module doc comments are unusually thorough (crypto.rs's semaphore documentation is exemplary). The deviations cluster in three spots of the tests/ layout convention — one module with no tests at all, five flat modules whose tests live in a shared crate-root `src/tests/` dir instead of per-module `tests/`, and registry contract tests nested under `math/tests/` — plus a few drifted doc comments, including a stale crate-level claim that most categories are "stubs".

## Findings

### 1. `scalar_resolver` is the only real module with no tests and no `tests/` dir
- **File:line:** `src/scalar_resolver.rs:1-145` (no `#[cfg(test)] mod tests;`; zero matches for `ScalarResolver`/`UserScalarLayer` anywhere under `src/**/tests/`)
- **Severity:** medium
- **Issue:** Every other functional module (agg, arrays, canonical, cast, compare, crypto, datetime, gen, math, null, text, validate, plus the lib root) follows the documented "one `tests/` directory per module" layout with topic-split coverage. `scalar_resolver.rs` — a public module consumed directly by shamir-engine/shamir-index/shamir-db on the hot filter path (2-layer user→builtin dispatch, `builtins_only()` OnceLock sharing) — has neither wiring nor tests in its home crate. Its core contract (user layer shadows builtins, fallback on miss, `unknown_function`, arity parity with `ScalarRegistry::call`, `get()` precedence) is exercised only indirectly via shamir-engine test fixtures (`resolver_with_user_scalar` in shamir-engine), not here.
- **Failure scenario:** A regression in dispatch precedence (e.g. builtin found before user fn, or `dispatch_entry` arity drift from `ScalarRegistry::call`) or in the shared `EMPTY` OnceLock would not be caught by `./scripts/test.sh -p shamir-funclib`; the TDD protocol in CLAUDE.md has no in-crate anchor for this module.
- **Suggested fix:** Add `src/scalar_resolver/tests/scalar_resolver_tests.rs` (+ `tests/mod.rs` manifest) covering user-first shadowing, builtin fallback, unknown-function, arity, `get()` precedence, and `builtins_only()` sharing one `UserScalarLayer` across calls; wire with `#[cfg(test)] mod tests;` at the end of `scalar_resolver.rs`.

### 2. Stale crate-level doc claims all non-math categories are "stubs"
- **File:line:** `src/lib.rs:12-13`
- **Severity:** low
- **Issue:** "`[`math`]` is the fully-implemented reference; the remaining categories are stubs to be populated by their owning agents." All remaining categories are fully implemented — 130+ registered functions across 13 categories, with per-category test suites (the `>= 130` assertion in `src/tests/register_builtins_tests.rs:17` confirms).
- **Failure scenario:** A reader trusting the crate's front-door doc may conclude categories are unimplemented, duplicate work, or distrust the module docs; comment-discipline rule ("do not touch comments unrelated to the task" cuts both ways — drifted comments mislead).
- **Suggested fix:** Replace the sentence with the current state, e.g. "all categories are implemented; each module's header documents its function catalogue."

### 3. Tests for five flat modules consolidated in `src/tests/` instead of per-module `tests/` dirs
- **File:line:** `src/tests/mod.rs:1-5`; host modules `src/encode.rs` (ends line 215), `src/object.rs` (189), `src/strings.rs` (435), `src/value_nav.rs` (140) carry no `#[cfg(test)] mod tests;`
- **Severity:** low
- **Issue:** CLAUDE.md mandates "One `tests/` directory per module", and the crate itself demonstrates the pattern 12 times (`agg.rs` + `agg/tests/`, `math.rs` + `math/tests/`, …). But `encode_tests.rs`, `object_tests.rs`, `strings_tests.rs`, and `value_nav_tests.rs` live in the crate-root `src/tests/` dir, and their host modules have no local test wiring — the association exists only through `lib.rs`'s root `mod tests`.
- **Failure scenario:** Module tests are not discoverable next to the module they cover; a future split of `strings.rs` (e.g. extracting the regex family into a file per one-file-one-export) will not carry its tests along; contributors copying the dominant in-crate pattern will strand these tests further.
- **Suggested fix:** Move each file to `<module>/tests/<module>_tests.rs` with a manifest-only `mod.rs`, and add `#[cfg(test)] mod tests;` to the four host modules. `register_builtins_tests.rs` legitimately stays under `src/tests/` — it tests the crate root (`lib.rs`).

### 4. `registry` contract tests nested under `math/tests/`
- **File:line:** `src/math/tests/registry_tests.rs:1` (wired via `src/math/tests/mod.rs:2`)
- **Severity:** low
- **Issue:** The file tests `crate::registry` (dispatch, arity, unknown-function, all `arg_*` extractors, all `v_*` constructors) — not the `/math` category — while `registry.rs` itself has no `#[cfg(test)] mod tests;`. Per the documented layout, these belong to the registry module's own `tests/` dir.
- **Failure scenario:** Test history is mis-attributed: `git log -- src/registry*` shows no tests; a refactor of `math.rs` looks test-covered for registry behaviour it does not own; the "at least one test per registered function" pairing between a module and its tests dir breaks.
- **Suggested fix:** Move to `src/registry/tests/registry_tests.rs`, wire from `registry.rs`, drop the entry from `math/tests/mod.rs`.

### 5. `use` statements inside function bodies
- **File:line:** `src/lib.rs:72` (`use std::sync::OnceLock;` inside `static_builtin()`), `src/scalar_resolver.rs:89` (inside `builtins_only()`), `src/crypto/tests/crypto_tests.rs:208-209` (`use std::sync::{Arc, Barrier}; use std::thread;` inside `argon2id_concurrency_cap_bounds_parallel_calls`), `src/crypto/tests/crypto_tests.rs:288` (`use argon2::{...}` inside `argon2id_matches_reference_and_is_deterministic`)
- **Severity:** low
- **Issue:** CLAUDE.md: "All `use` statements live in the file header … never inside a function or block body," with three documented exceptions (test-mod `use super::*;`, single-method trait-import collision, cfg-gated bodies). None applies here. Hoisting is trivially safe in all four cases.
- **Failure scenario:** None functional; it is drift from a written rule and an idiom inconsistent with the rest of the crate, where everything else is hoisted.
- **Suggested fix:** Hoist `OnceLock` to the headers of `lib.rs` and `scalar_resolver.rs`; hoist the std/argon2 imports to the top of `crypto_tests.rs`.

### 6. Doc/code drift: `random()` documented as returning `F64`, actually `Dec`
- **File:line:** `src/gen.rs:15` (vs `src/registry.rs:303-310`, `v_f64` → `QueryValue::Dec`)
- **Severity:** nit
- **Issue:** gen.rs header: "`random()` takes 0 args and returns an `F64` in `[0.0, 1.0)`." The implementation routes through `v_f64`, which intentionally stores `QueryValue::Dec` (decimal-first value model), so the type a caller observes is `dec`.
- **Suggested fix:** Correct the doc to "returns a `Dec` in [0, 1) (via `v_f64`)".

### 7. Doc drift: encode.rs header lists `str_escape_chars` as a registered function name
- **File:line:** `src/encode.rs:4-6` (registered name is `json_escape`, line 139)
- **Severity:** nit
- **Issue:** The header catalogue names `html_escape str_escape_chars` as registered functions, but `register()` registers `json_escape` for the `str_escape_chars` implementation (the conventions bullet at line 14 gets this right, contradicting the header above it).
- **Suggested fix:** Update the header list to `… html_escape json_escape to_json parse_json`.

### 8. `scalar_resolver` doc references `builtin_scalars()` without a path
- **File:line:** `src/scalar_resolver.rs:3`
- **Severity:** nit
- **Issue:** The backticked `builtin_scalars()` resolves to nothing in this crate — the in-crate function is `crate::static_builtin()`; a `pub fn builtin_scalars()` exists only in `shamir-wasm-host` (`crates/shamir-wasm-host/src/scalar.rs:20`).
- **Suggested fix:** Reference `crate::static_builtin()` and note the embedder-facing alias, e.g. "`static_builtin()` (published to embedders as `shamir_wasm_host::builtin_scalars()`)".

### 9. Category-header phrasing drift about folder qualification
- **File:line:** e.g. `src/math.rs:4`, `src/arrays.rs:4`, `src/cast.rs:3`, `src/datetime.rs:4`, `src/encode.rs:3`, `src/object.rs:3`, `src/strings.rs:6`, `src/text.rs:6`, `src/validate.rs:3` (vs the correct `src/gen.rs:3` and `src/null.rs:3`)
- **Severity:** nit
- **Issue:** Nine category headers say "Functions registered (plain names, no folder prefix)", which is true only at `register()` time — `register_builtins` folder-qualifies every one of them (`math/abs`, `arrays/min`, …). gen/null document the qualification; the other nine read as if the wire names were unqualified.
- **Suggested fix:** Adopt the gen/null phrasing ("plain names, folder-qualified to `X/…` by `crate::register_builtins`") across the nine headers.

### 10. Single-line JSON literals in tests
- **File:line:** `src/tests/encode_tests.rs:157`, `src/validate/tests/validate_tests.rs:186`
- **Severity:** nit
- **Issue:** Discipline rule: "In tests, JSON literals are always multi-line and indented for readability." Both sites use one-line raw strings (`r#"{"x": 1, "y": "hello"}"#`, `r#"{"a": [1, 2, true, null], "b": "xA"}"#`).
- **Failure scenario:** None; the payloads are short enough to read inline, but they contradict the letter of the rule.
- **Suggested fix:** Either reformat as multi-line indented literals or amend the rule to exempt short inline payloads.

### 11. `registry.rs` and `agg.rs` stretch "one file = one primary export"
- **File:line:** `src/registry.rs:20-325` (`ScalarError`, `ScalarResult`, `ScalarFn`, `FnEntry`, `ScalarRegistry` + 16 public `arg_*`/`v_*` free fns), `src/agg.rs:45-856` (`Aggregator`, `AggFactory`, `AggRegistry`, `DistinctWrapper`, `percentile`, `string_agg` + ~18 private aggregator impls)
- **Severity:** nit
- **Issue:** CLAUDE.md allows a "closely-coupled group", and both files qualify — one ABI + one registration path each; agg's per-aggregator structs are private. Still, the shared extractor/constructor half of `registry.rs` (`arg_*`/`v_*`, lines 200-325) is a coherent standalone unit consumed by every category, and `registry_tests.rs` (finding 4) already treats it as a distinct topic.
- **Failure scenario:** None today; file breadth makes blame and diffs noisier as categories grow.
- **Suggested fix (optional):** Move the `arg_*`/`v_*` helpers into a sibling file (e.g. `registry/args.rs`) re-exported from `registry` so existing `use crate::registry::{…}` sites are unchanged; leave `agg.rs` as-is.

## Conformance notes (no action)

- **mod.rs re-export-only:** all 13 `mod.rs` files contain only `pub mod …;` lines — exact match to the mandated manifest template.
- **No inline test blocks:** zero occurrences of `mod tests {` anywhere under `src/`.
- **Wire-in convention:** 12 category modules + `lib.rs` correctly carry `#[cfg(test)] mod tests;` at end-of-file.
- **Bench convention:** `benches/distinct_arrays.rs` uses `bench_scale_tool::Harness` (no Criterion), consistent with the 2026-07-07 migration; `Cargo.toml` sets `doctest = false` per the workspace ban.
- **Error handling:** every fallible path returns `Result<_, ScalarError>` with stable machine codes; no `panic!`/`unwrap` on library paths (the two `unwrap()`s in `validate.rs:27-45` guard `LazyLock<Regex>` construction of compile-time-constant patterns); no `anyhow` in library APIs.
