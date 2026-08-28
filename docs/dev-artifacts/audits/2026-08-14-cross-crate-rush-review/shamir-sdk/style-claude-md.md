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
