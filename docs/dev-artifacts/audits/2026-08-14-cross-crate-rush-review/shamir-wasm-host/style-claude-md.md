# shamir-wasm-host -- Style & CLAUDE.md structural conformance

## Summary

The crate is strongly conformant on the structural conventions: every `mod.rs` (`lib.rs`, `src/wasm/mod.rs`, `src/tests/mod.rs`) is re-exports/manifest only, tests follow the documented `tests/` layout exactly (topic-split files, manifest-only `tests/mod.rs`, `#[cfg(test)] mod tests;` wired from the parent `mod.rs`, zero inline `#[cfg(test)] mod tests` blocks), every `scc::*::len()` allow carries the mandated `// O(N) ack:` comment, and there is no lock/Mutex/parking_lot or TODO/FIXME debris. The real conformance gaps are concentrated in two of the five lens areas: **imports-at-top** is violated in 7 places (2 in `compile.rs`, 5 in `tests/compile_tests.rs`), and **comment discipline** has one misleading doc ("reuses the same logic" above a verbatim duplicate of `env_policy::glob_matches`) plus one copy-pasted doc-comment block. Remaining findings are borderline one-file-one-export judgments and minor manifest/doc drift.

## Findings

### 1. Mid-body `use` statements violate the documented "Imports at the top" rule (7 sites)

**File:line:** `crates/shamir-wasm-host/src/compile.rs:574`, `:582`; `crates/shamir-wasm-host/src/tests/compile_tests.rs:111`, `:128`, `:140`, `:153`, `:169`
**Severity:** medium

**Issue:** CLAUDE.md ("📦 Imports at the top") requires all `use` statements in the file header, with three narrow exceptions; none apply here:

- `compile.rs:574` and `:582` — `use std::io::Read;` sits inside the two `std::thread::spawn` pipe-drain closures. `Read` is a trait imported solely for one method, but the documented exception requires a top-level import *collision* plus a one-line collision comment — there is no other `Read` in scope in `compile.rs`, and no comment.
- `compile_tests.rs` (5 occurrences) — `use crate::compile::test_find_forbidden_macro;` is re-declared inside five separate test functions. It is a function (not a trait), there is no collision, and the cfg-gating exception does not apply because the entire test file is already compiled only under `cfg(test)` — a header import is valid.

**Failure scenario:** none functional; each new test in `compile_tests.rs` tends to copy the local-import pattern (it already repeated 5×), so the violation propagates, and `fmt`/`clippy -D warnings` never catch it — only convention review does.

**Suggested fix:** hoist `use std::io::Read;` into `compile.rs`'s header (next to `use std::fs;`) and hoist a single `use crate::compile::test_find_forbidden_macro;` to the top of `compile_tests.rs`; delete the seven in-body imports.

### 2. `glob_matches` duplicated in `net_gateway.rs` under a doc comment falsely claiming reuse

**File:line:** `crates/shamir-wasm-host/src/net_gateway.rs:483-514` vs `crates/shamir-wasm-host/src/env_policy.rs:75-106`
**Severity:** medium

**Issue:** `net_gateway.rs` carries a private `glob_matches` whose doc says *"Tiny `*`-only glob matcher — reuses the same logic as `EnvPolicy`."* It does not reuse anything: it is a body-for-body copy of `env_policy::glob_matches` — which is `pub(crate)` and directly importable from this same crate. This is both a misleading comment (the comment-discipline lens) and a real DRY break in security-relevant logic: the copy is the matcher behind the SSRF egress allowlist (`check_host_allowed`, `host_has_exact_match`), while the `env_policy` copy gates `env.*` secret seeding.

**Failure scenario:** a matcher fix (e.g. the anchor rules for multi-`*` or non-`*`-terminated patterns — non-trivial logic, note the `i == 0` / `cursor != text.len()` conditions) lands in the `env_policy` copy, the one with direct unit tests (`env_policy_tests.rs`). The SSRF guard silently keeps the old behavior, and because the comment asserts the two are "the same logic", a maintainer has no reason to look for a second copy. Secret-grant masking and egress allowlisting then disagree on what a pattern matches.

**Suggested fix:** delete `net_gateway.rs`'s private `glob_matches` and `use crate::env_policy::glob_matches;` instead; reword the doc comment to state the shared single implementation. (Consider whether the matcher deserves its own tiny module or a `glob`-topic test file shared by both consumers.)

### 3. Verbatim-duplicated doc-comment block on `host_call`

**File:line:** `crates/shamir-wasm-host/src/wasm/host_call.rs:16-27`
**Severity:** low

**Issue:** Lines 16-21 ("Host implementation of `call(...)` … propagating as the caller's `FunctionError::Compute`.") are repeated verbatim at lines 22-27 before the `# Borrow dance across await` section — a copy-paste remnant that rustdoc renders twice.

**Failure scenario:** cosmetic only, but future edits to one copy will leave the stale twin in place (exactly how it likely arose).

**Suggested fix:** delete the duplicated six lines.

### 4. `net_gateway.rs` carries two primary concerns (one-file-one-export, borderline)

**File:line:** `crates/shamir-wasm-host/src/net_gateway.rs:24-61` (trait + DTOs) vs `:63-514` (SSRF guard)
**Severity:** low

**Issue:** CLAUDE.md's "One file = one primary export" allows a *closely-coupled group*, but this 514-line file mixes the `NetGateway` trait and its wire DTOs (`HttpRequest`, `HttpResponse`, plus `ResolvedPin`) with ~350 lines of self-contained pure guard logic (`check_host_allowed`, `check_url_allowed`, `check_url_allowed_resolved`, `parse_url`, `canonicalize_ip`, `parse_inet_aton`, `parse_inet_component`, `is_private_or_loopback_*`, `glob_matches`). The guard never touches the trait; it is consumed by the facade's `CurlNetGateway` and has its own dedicated test topic in `net_gateway_tests.rs`.

**Failure scenario:** none directly; the cost is diff/blame granularity — a guard tweak and a DTO change land in the same file.

**Suggested fix:** optionally split the egress guard into e.g. `src/net_guard.rs`, keeping `lib.rs`'s flat `pub use net_gateway::{...}` surface intact (re-export from both). Note for calibration: `context.rs` (BatchContext/GlobalVars/FnCtx/FnBatch) and `meta.rs` (4 types) also define multiple public types, but they read as documented closely-coupled groups (FnCtx holds `Arc<GlobalVars>`, FnBatch holds `Arc<BatchContext>`; meta.rs is doc-framed as one catalogue-metadata family) — no action needed there.

### 5. Unused direct dependency `serde` in Cargo.toml

**File:line:** `crates/shamir-wasm-host/Cargo.toml:14`
**Severity:** low

**Issue:** `serde = { version = "1.0.217", features = ["derive"] }` is declared but never used anywhere in `src/` (the only "serde" matches are two doc comments in `compile.rs` describing `shamir-sdk`'s own transitive deps). Dead manifest surface, with the `derive` feature pulling proc-macro weight into this crate's build for nothing. Flagged as adjacent to this lens (manifest/code structural mismatch); a sibling dependency-themed reviewer may cover it too.

**Failure scenario:** none functional; future readers assume serde is part of the crate's data path (it is not — params/results are msgpack via `shamir-types`).

**Suggested fix:** remove the dependency, or add a comment justifying it the way the other non-obvious deps (`wait-timeout`, `wasmparser`, `wat`) are justified.

### 6. Security-bearing host imports untested in this crate's own `tests/` (test-locality)

**File:line:** `crates/shamir-wasm-host/src/wasm/host_globals.rs` (whole file); cf. `crates/shamir-db/tests/functions_lifecycle.rs:1116-1280`
**Severity:** low

**Issue:** `host_global_set`'s unconditional `env.*` write-protection trap and `host_global_get`'s secret-grant gating (slice 9) are security behaviors of *this* crate, yet this crate's `src/tests/` has no test exercising them — coverage lives only in the downstream consumer's integration suite (`shamir-db/tests/functions_lifecycle.rs`: `secret_grant_gates_env_read` and the `global_set("env.…")` trap test). `wasm_sanitizer_tests.rs` compiles modules importing all sanctioned names but never calls them, and `wasm_tests.rs`/`nested_actor_tests.rs` exercise only `shamir_call`. The crate demonstrably can test host imports end-to-end without the facade (nested_actor_tests does it for `call`), so this is locality, not capability.

**Failure scenario:** a regression in the grant check or the `env.` prefix trap would pass this crate's suite and only be caught (or missed, if the e2e test is skipped on a toolchain-less host — `functions_lifecycle.rs` prints SKIP paths) by another crate's tests.

**Suggested fix:** add a `tests/host_globals_tests.rs` topic file driving a small WAT guest that imports `global_get`/`global_set`: granted vs ungranted `env.*` read, non-`env.` key ungated, and the `env.*` write trap.

### 7. `wasm/mod.rs` doc list omits the sanitizer re-exports

**File:line:** `crates/shamir-wasm-host/src/wasm/mod.rs:3-5` vs `:18`
**Severity:** nit

**Issue:** The module doc says "Re-exports the public surface:" and lists `WasmEngine`/`WasmLimits` and `WasmFunction`, but line 18 also re-exports `verify_wasm_module` and `SANCTIONED_HOST_IMPORTS` (the crate's security-ABI surface) — unlisted.

**Failure scenario:** none; minor doc drift.

**Suggested fix:** add a `* verify_wasm_module / SANCTIONED_HOST_IMPORTS — import-allowlist sanitizer (wasm_sanitizer).` bullet.
