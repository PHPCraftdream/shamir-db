# shamir-sdk-macros -- Error handling & resource lifecycle

## Summary

The crate's entire error surface is compile-time validation of consumer signatures, and every one of its ~15 validation branches reports failure with `assert!`/`panic!` instead of a spanned `syn::Error` (only `parse_macro_input!` follows the graceful path), which contradicts CLAUDE.md's "avoid `panic!` outside programmer-bug invariants" rule. In the generated guest code, every user `Err` is flattened via `e.to_string()` into a panic-trap that the host classifies as `FunctionError::Compute`, collapsing the SDK's typed `thiserror` taxonomy at the ABI boundary (acknowledged by a TODO, but live behavior today for all three `Result`-returning macros). There are zero tests in this crate and no negative/error-path coverage anywhere in the workspace -- not even for the two macros (`validator`, `function`) that have no coverage at all. Resource lifecycle of the generated code is otherwise sound and documented (the intentional bump-allocator leak), though the unvalidated `i32` ABI lengths leave open panic/UB paths and `#[validator]` silently swallows param-extraction errors.

## Findings

### 1. All signature validation panics via `assert!`/`panic!` instead of `syn::Error::to_compile_error()`

- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:51-54, 57-60, 63-72, 81` (`validator`); `183-190, 193-202, 211` (`function`); `314-323, 326-335, 344` (`procedure`); `471-481, 486-490, 495-504, 513` (`scalar`)
- **Severity:** medium
- **Issue:** Consumer signature mistakes (non-async fn, wrong arity, wrong return type, `Ctx` in a scalar, receiver-style args) are macro-consumer *input* errors, not macro-programmer invariants, yet each is reported with `assert!`/`panic!`. CLAUDE.md's error-handling section is normative: "Avoid `panic!` outside `unreachable!()` / invariant violations that mean a programmer bug." A rustc-caught proc-macro panic points at the macro invocation with a backtrace note and no span on the actual offending element, and it is inconsistent with the crate's own first line of defense (`parse_macro_input!`, lib.rs:45/177/308/465), which already emits proper spanned compile errors for parse failures.
- **Failure scenario:** An author writes `pub fn check(record: Value, old: Option<Value>, ctx: Ctx) -> Validation` (missing `async`). rustc emits `proc macro panicked` + message + "the backtrace..." noise pointing at `#[shamir_sdk::validator]`, rather than a crisp error spanned on the `fn` signature.
- **Suggested fix:** In each validation branch, `return syn::Error::new_spanned(&fn_item.sig.asyncness/ident/output, "...").to_compile_error().into();` (short-circuiting expansion, as `parse_macro_input!` already does). Reserve `panic!`/`unreachable!()` only for genuinely impossible arms.

### 2. Generated `Err` path flattens typed user errors into a panic-trap the host misclassifies as `FunctionError::Compute`

- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:271-273` (`function`), `398-400` (`procedure`), `563-565` (`scalar`); acknowledged TODO at lib.rs:251; root mechanism `crates/shamir-sdk/src/__rt.rs:64-69` (`trap` = `panic!("shamir function error: {msg}")`)
- **Severity:** medium
- **Issue:** `Err(e) => shamir_sdk::__rt::trap(&e.to_string())` stringifies a typed `thiserror` error and delivers it through a panic. The host maps *any* trap to `FunctionError::Compute` (`__rt.rs:63`), so a user-level failure (e.g. `Error::MissingParam("n")`) is indistinguishable from a genuine guest compute crash. Error taxonomy is destroyed at exactly the boundary where the host needs it, and the error path also runs through `panic!` machinery (formatting + unwinding/abort) rather than a structured result channel.
- **Failure scenario:** A `#[function]` returns `Err(...)` on bad params; the DB host records/returns `FunctionError::Compute` for a routine user mistake, polluting compute-error metrics/alerts and stripping the error kind from callers.
- **Suggested fix:** Land the slice-4 TODO noted at lib.rs:251: emit a machine-readable envelope over the existing `(ptr, len)` ABI (e.g. tag byte or reserved export distinguishing `Ok`/`UserErr`/`ComputeErr`, with the message as msgpack/UTF-8). Interim minimal step: prefix the trap message (`user-error: {e}`) so hosts can classify on string without breaking the ABI.

### 3. Zero tests in-crate; no error-path coverage for any validation branch anywhere in the workspace

- **File:line:** whole crate (no `tests/` dir, no `#[cfg(test)]`); `crates/shamir-sdk-macros/Cargo.toml` (no dev-dependencies); only cross-crate coverage is `crates/shamir-sdk/tests/scalar_compile_pass.rs` and `procedure_compile_pass.rs` (happy-path compile-only); no `trybuild` anywhere in the workspace
- **Severity:** medium
- **Issue:** CLAUDE.md's TDD protocol and test-organisation rules demand failing-test-first and a `tests/` directory per module. This crate has none. Concretely untested: all ~15 assert branches of finding 1 (their messages and their *existence*), the `#[validator]` and `#[function]` macros (not compiled by any test in the workspace -- the two existing compile-pass tests cover only `scalar`/`procedure`), and the generated `shamir_call` `Ok`/`Err` runtime branches (the `Err => trap` path of findings 2/4/6 has zero coverage).
- **Failure scenario:** A refactor silently drops the Ctx-purity assert in `#[scalar]` or changes a validation message; no test fails, and the regression ships.
- **Suggested fix:** Add `trybuild` as a dev-dependency with UI tests under `crates/shamir-sdk-macros/tests/` (integration tests, so the `#[no_mangle]` symbols stay isolated per the pattern the sibling tests document) covering: sync fn, wrong arity (per macro), wrong return type, `Ctx`-in-scalar; add compile-pass expansions for `#[validator]` and `#[function]`; add one runtime test driving the generated `shamir_call` through both the `Ok` and `Err` arms.

### 4. `#[validator]` generated code silently swallows param-extraction errors -- missing vs malformed payload indistinguishable

- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:129-132` (`record`: `Err(_) => Value::Null`), `135-139` (`old_record`: `Err(_) => None`); compounded by `crates/shamir-sdk/src/__rt.rs:11-16` (`decode_params` maps malformed/non-map payloads to an empty `Params`)
- **Severity:** low
- **Issue:** Two silent swallows in series: a garbage/malformed payload yields an empty `Params` (sibling crate), then `params.get("record")`'s `Err` is discarded, so the author's validator receives `record = Value::Null` and reports a misleading domain error (e.g. "empty_record") for what is really a transport/encoding failure. The `Err(_)` discards the cause entirely; the conflation is not documented as a design decision.
- **Failure scenario:** Host-side msgpack encoding bug corrupts the payload; every validator rejects with per-record domain errors instead of one loud "malformed params" signal, sending debuggers down the wrong path.
- **Suggested fix:** Distinguish absence from malformed -- either have the generated code detect an undecodable payload up front (early `trap`/`Validation::record_error("malformed_params")`) or have `decode_params` surface decode failure; at minimum document the intentional `Null`-on-error conflation in the `#[validator]` doc comment.

### 5. Return-type validation is inconsistent across the four macros; equivalent spellings are spuriously rejected

- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:63-72` (`validator`: exact `"Validation"` only), `193-202` (`function`: exact `"Result<Value>"` or `"core::result::Result<Value,Error>"` only) vs `326-335`/`495-504` which use `is_result_value_return` (`411-420`); the normalizer itself misses `std::result::`
- **Severity:** low
- **Issue:** String-munging type checks disagree between macros: `#[function]` with `-> shamir_sdk::Result<Value>` fails (not one of the two exact strings), `#[validator]` with `-> shamir_sdk::Validation` fails, while the identical shapes pass under `#[procedure]`/`#[scalar]` via the normalizing helper; conversely `is_result_value_return` accepts/normalizes more forms but not `std::result::Result<...>`.
- **Failure scenario:** An author writes the same function signature style that works with `#[scalar]` under `#[function]` and gets a spurious "must return Result<Value>" panic (styled as a proc-macro panic per finding 1).
- **Suggested fix:** Route all four macros through one shared, tested checker; extend it to strip `std::result::` (and ideally resolve the last path segment syntactically instead of string `replace`), and apply the normalized form to `Validation` too.

### 6. Generated ABI functions trust `i32` inputs unvalidated: negative `len` yields alloc-abort or UB

- **File:line:** `shamir_alloc`: `crates/shamir-sdk-macros/src/lib.rs:109-114, 239-244, 370-375, 537-542` (`vec![0u8; len as usize]`); `shamir_call`: `lib.rs:122-124, 255-257, 383-385, 549-551` (`from_raw_parts(ptr as *const u8, len as usize)`)
- **Severity:** low
- **Issue:** Neither generated export validates its `i32`s before use. `shamir_alloc(-1)` requests `vec![0u8; 0xFFFF_FFFF_FFFF_FFFF]` -> allocation failure -> abort (WASM trap) rather than a graceful signal; `shamir_call` with a negative `len` constructs a `usize::MAX`-length slice -> undefined behavior on the first read, before msgpack decoding could bound anything. The `// Safety:` comments lean entirely on the host contract with no cheap runtime guard backing it.
- **Failure scenario:** A host-side ABI bug (sign-extension slip when packing `(ptr, len)`) passes `len = -1`; the guest dies with an opaque allocator abort or reads wild memory, instead of trapping with a clear "bad ABI length" message.
- **Suggested fix:** One-line guards at each boundary: `if len < 0 { return 0; }` in `shamir_alloc`; in `shamir_call`, reject `len < 0` (and a null `ptr` when `len > 0`) with `trap("shamir: bad ABI length")` before `from_raw_parts`.

### 7. Generated `block_on` has no deadline/fuel guard: a genuinely-Pending future spins hot forever on the failure path

- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:144, 264, 391, 556` (emitted `shamir_sdk::__rt::block_on(...)` calls); root mechanism `crates/shamir-sdk/src/__rt.rs:49-59` (spin-on-`Pending`)
- **Severity:** low (borderline sibling-crate scope -- root lives in `shamir-sdk::__rt`, but the macro chooses to emit the unguarded call)
- **Issue:** The generated `shamir_call` drives the author's future with a `block_on` that busy-spins on `Pending`. The macros advertise `async` entry points (all doc examples are async), and slice-4 host imports are planned; once a future can legitimately pend, the error/liveness path is a 100%-CPU infinite spin inside the guest with no deadline, no trap message, and no resource cleanup -- it only dies via host fuel exhaustion with a generic trap.
- **Failure scenario:** Post-slice-4, an author's `#[procedure]` awaits a host import whose waker is the no-op one; the guest burns its entire fuel budget spinning, and the host reports an opaque compute trap instead of "future never resolved".
- **Suggested fix:** When emitting `block_on`, wrap it with a bounded poll/step budget (trap with "validator/procedure timed out after N polls") -- or, once real wakers land in `__rt`, park instead of spin. Coordinate the fix with the `shamir-sdk` owner since the primitive is shared.

## Reviewed and sound (no action)

- `parse_macro_input!` is used correctly at all four entry points (graceful spanned errors for parse failures).
- The intentional `shamir_alloc`/`leak_result` leaks are deliberate bump-allocator design, documented at each emission site; no cleanup is expected on those paths in short-lived WASM guests.
- The generated wrapper bodies otherwise propagate `Result` faithfully and use `?`-friendly signatures (`shamir_sdk::Result<Value>`); no `unwrap()`/`expect()` appears in macro or generated code.
