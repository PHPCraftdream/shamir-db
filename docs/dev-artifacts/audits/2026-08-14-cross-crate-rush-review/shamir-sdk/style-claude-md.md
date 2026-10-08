<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The 17 function-body imports and documentation hygiene issues remain. They are convention/documentation matters, not independent high-severity runtime defects.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 4 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Function-body `use` imports in `src/tests/` violate the "Imports at the top" rule

Status: `confirmed-open`. Current risk: `low`.

All 17 cited imports remain: 15 in value_tests and two in validation_tests. No documented collision/cfg exception justifies these local imports. This is a style-only issue with no runtime failure mechanism.

Evidence: [crates/shamir-sdk/src/tests/value_tests.rs:146](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L146); [crates/shamir-sdk/src/tests/value_tests.rs:279](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L279); [crates/shamir-sdk/src/tests/value_tests.rs:401](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L401); [crates/shamir-sdk/src/tests/validation_tests.rs:239](../../../../../crates/shamir-sdk/src/tests/validation_tests.rs#L239); [crates/shamir-sdk/src/tests/validation_tests.rs:305](../../../../../crates/shamir-sdk/src/tests/validation_tests.rs#L305).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Stale slice-jargon comments in `__rt::block_on` misdescribe what the SDK supports

Status: `confirmed-open`. Current risk: `low`.

The pure-functions-only and future-slice-4 comments are unchanged despite all four macro kinds using block_on and host imports already existing. The accurate invariant is guest-side synchronous imports, not that every author-written future must be Ready.

Evidence: [crates/shamir-sdk/src/__rt.rs:34](../../../../../crates/shamir-sdk/src/__rt.rs#L34); [crates/shamir-sdk/src/__rt.rs:54](../../../../../crates/shamir-sdk/src/__rt.rs#L54); [crates/shamir-sdk-macros/src/lib.rs:144](../../../../../crates/shamir-sdk-macros/src/lib.rs#L144); [crates/shamir-sdk-macros/src/lib.rs:391](../../../../../crates/shamir-sdk-macros/src/lib.rs#L391).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `value.rs` module doc points at a test path that does not exist

Status: `confirmed-open`. Current risk: `nit`.

The doc still points to tests/value_tests.rs; conformance tests are registered from src/tests/value_tests.rs.

Evidence: [crates/shamir-sdk/src/value.rs:9](../../../../../crates/shamir-sdk/src/value.rs#L9); [crates/shamir-sdk/src/tests/mod.rs:2](../../../../../crates/shamir-sdk/src/tests/mod.rs#L2); [crates/shamir-sdk/src/tests/value_tests.rs:14](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L14).

Grouping/duplicate: `SUMMARY.md#7.3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `__rt` doc claims "not part of the public SDK surface" while the module is fully `pub` with no `#[doc(hidden)]`

Status: `confirmed-open`. Current risk: `nit`.

Public visibility and missing doc(hidden) remain. Macro-generated consumer code needs the public path; clarify unsupported direct use and support policy rather than asserting that public implementation visibility itself contradicts internal API intent.

Evidence: [crates/shamir-sdk/src/lib.rs:18](../../../../../crates/shamir-sdk/src/lib.rs#L18); [crates/shamir-sdk/src/__rt.rs:3](../../../../../crates/shamir-sdk/src/__rt.rs#L3); [crates/shamir-sdk-macros/src/lib.rs:259](../../../../../crates/shamir-sdk-macros/src/lib.rs#L259).

Grouping/duplicate: `SUMMARY.md#5.7`. This row is not another independent defect.

<a id="review-summary"></a>

### Claim Summary — Strong structural conformance: manifest-only tests, header imports, and closely-coupled public types

Status: `not-applicable`. Current risk: —.

The cited non-finding remains supported: tests/mod.rs is a manifest, lib.rs wires tests, no inline test modules exist, implementation imports are at file/module headers, and paired API types are closely coupled. Test-body imports are the separately identified exception.

Evidence: [crates/shamir-sdk/src/tests/mod.rs:1](../../../../../crates/shamir-sdk/src/tests/mod.rs#L1); [crates/shamir-sdk/src/lib.rs:62](../../../../../crates/shamir-sdk/src/lib.rs#L62); [crates/shamir-sdk/src/host_imports.rs:23](../../../../../crates/shamir-sdk/src/host_imports.rs#L23); [crates/shamir-sdk/src/context.rs:58](../../../../../crates/shamir-sdk/src/context.rs#L58); [crates/shamir-sdk/src/http.rs:51](../../../../../crates/shamir-sdk/src/http.rs#L51).

## Corrections and qualified non-findings

- The proposed replacement comment must not claim all bodies are necessarily Ready or that a spin itself proves a programming bug; supported execution policy needs explicit documentation.
- doc(hidden) hides generated-support APIs from normal rustdoc but does not make them private or automatically semver-exempt.
- Preserve style-only severity and separation from substantive runtime changes.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk -- Style & CLAUDE.md structural conformance

## Summary

The crate is in strong structural conformance with CLAUDE.md: there is exactly one `mod.rs` (`src/tests/mod.rs`) and it is a manifest-only re-export of two topic files; tests are wired via `#[cfg(test)] mod tests;` in `lib.rs` (no inline `#[cfg(test)] mod tests { ... }` blocks anywhere); every non-test source file keeps all `use` statements in its header (the per-`cfg` `use crate::Value;` in `host_imports.rs` sits at the enclosing `mod imp` header, which the rule explicitly allows); and each file's multiple public types (`Ctx`+`Batch`, `Db`+`Table`, `HttpRequest`+`HttpResponse`, `Validation`+`ValidationError`+`IntoFieldPath`) qualify as "closely-coupled groups" under the one-file-one-export carve-out. The real deviation is concentrated in `src/tests/`: 17 function-body `use` imports across the two test files, with none of the three documented exceptions applying. A few doc comments have drifted from the code they describe.

## Findings

### 1. Function-body `use` imports in `src/tests/` violate the "Imports at the top" rule

- **File:line:** `crates/shamir-sdk/src/tests/value_tests.rs:146, 177, 193, 225, 246, 279-280, 298, 312-313, 331, 345, 366, 385, 401`; `crates/shamir-sdk/src/tests/validation_tests.rs:239, 305` (17 sites total)
- **Severity:** low
- **Issue:** CLAUDE.md §"Imports at the top" is unconditional: "All `use` statements live in the file header ... never inside a function or block body," with exactly three documented exceptions. None applies here: these are not `use super::*;` in a test mod; the only trait import (`use std::str::FromStr;` at value_tests.rs:280, 313) is a single-method call with *no* top-level collision and *no* justifying comment (the exception requires both); and nothing is macro-generated or `cfg`-gated. Repeated offenders: `use shamir_types::types::common::new_map;` / `new_set;` (11 sites), `use shamir_types::types::value::QueryValue;` (validation_tests.rs:239, 305), `use rust_decimal::Decimal;`, `use num_bigint::BigInt;`, `use std::str::FromStr;`. None of these names collide at file scope, so hoisting is purely mechanical.
- **Failure scenario:** none at runtime — the cost is convention drift: the documented rule erodes case-by-case ("tests are different"), making the next mid-body import in non-test code harder to argue against, and `git blame`/greppability of imports degrades.
- **Suggested fix:** hoist all 17 imports to the file headers, merging with the existing header block (`use shamir_types::types::common::{new_map, new_set};`, `use shamir_types::types::value::QueryValue;`, `use rust_decimal::Decimal;`, `use num_bigint::BigInt;`, `use std::str::FromStr;`). Land as a separate `style:` commit per the CLAUDE.md sweep rule.

### 2. Stale slice-jargon comments in `__rt::block_on` misdescribe what the SDK supports

- **File:line:** `crates/shamir-sdk/src/__rt.rs:32-35, 53-57`
- **Severity:** low
- **Issue:** The doc comment says the no-op-waker executor "Works because pure functions (the only kind this slice supports) are `Ready` on the first poll," and the `Poll::Pending` arm says "If a future genuinely needs async I/O (slice 4 host imports), this will spin. For now, a tight loop is correct." Both statements are stale: `shamir-sdk-macros` routes **all four** kinds (`#[scalar]`, `#[function]`, `#[procedure]`, `#[validator]`) through `__rt::block_on` (macros crate `lib.rs:144, 264, 391, 556`), and the "future" slice-4 host imports already exist (`host_imports.rs`, slices 8b/8c) — as *synchronous* calls, which is the load-bearing fact for the spin-safety claim, but the comment never says so.
- **Failure scenario:** a maintainer reading only `__rt.rs` concludes `block_on` is scalar-only scaffolding and either (a) "fixes" it to a real executor unnecessarily, or (b) adds a genuinely-`.await`ing future believing the comment covers it, getting a silent WASM busy-spin.
- **Suggested fix:** rewrite the comment to the current reality, e.g. "All four generated kinds run through `block_on`; every host import (`host_imports.rs`) is a synchronous extern call, so no body yields `Pending` — a spin on `Pending` is a programming bug, not a wait."

### 3. `value.rs` module doc points at a test path that does not exist

- **File:line:** `crates/shamir-sdk/src/value.rs:9`
- **Severity:** nit
- **Issue:** The module doc says "(see conformance tests in `tests/value_tests.rs`)". The crate-root `tests/` directory contains only `procedure_compile_pass.rs` / `scalar_compile_pass.rs`; the conformance tests live at `src/tests/value_tests.rs`. As written, the path resolves to nothing.
- **Failure scenario:** a reader following the pointer greps/opens `crates/shamir-sdk/tests/` and concludes the claimed conformance coverage doesn't exist.
- **Suggested fix:** change the reference to `crate::tests::value_tests` (or `src/tests/value_tests.rs`).

### 4. `__rt` doc claims "not part of the public SDK surface" while the module is fully `pub` with no `#[doc(hidden)]`

- **File:line:** `crates/shamir-sdk/src/__rt.rs:1-3` vs `crates/shamir-sdk/src/lib.rs:18`
- **Severity:** nit
- **Issue:** `__rt.rs` states "Nothing in this module is part of the public SDK surface," yet `lib.rs` declares `pub mod __rt;` and every helper is `pub fn` (necessarily so: macro-generated code references `shamir_sdk::__rt::…` from user crates). The claim and the visibility contradict each other without the conventional marker.
- **Failure scenario:** tooling (rustdoc, `cargo public-api`-style checks, or a reviewer triaging "is this a breaking change?") treats `__rt` as a supported public API because it is publicly reachable and undocumented as internal-in-name-only; users `use shamir_sdk::__rt::leak_result` and get no signal it's off-limits.
- **Suggested fix:** add `#[doc(hidden)]` to the `pub mod __rt;` declaration in `lib.rs` (keeps the generated-code path stable while hiding it from docs), and reword the module doc to "public only so macro-generated code can reach these paths; not for direct use."

</details>
