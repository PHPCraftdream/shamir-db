# P1 stage H10 — p1-sdk-macros

## Goal

SDK macros: patterns, typed errors, compile errors, unsafe slice. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H10): Parameter patterns are bound, not reused as expressions; typed user errors reach the host as function errors; validator payload not retained across await; raw identifiers handled; signature errors are compile errors; slice construction validated.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-SDK-MACROS-9AC2B50C — User parameter patterns are re-used as call-position expressions (`mut x`, `_`, `ref x` break the expansion)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk-macros/src/lib.rs:87; crates/shamir-sdk-macros/src/lib.rs:112; changed by: none
- R0 note (includes the fix site): OBSERVED: Emitters still clone user parameter patterns and interpolate those same tokens into call expressions; qualification and return-type changes do not extract argument bindings. INFERRED: mut/ref patterns and wildcards still produce invalid forwarding expressions. Fix site: crates/shamir-sdk-macros/src/lib.rs needs distinct generated binding identifiers and pattern-aware forwarding.

### RV-SDK-MACROS-BB6378E2 — Generated `Err` path flattens typed user errors into a panic-trap the host misclassifies as `FunctionError::Compute`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk-macros/src/lib.rs:302; crates/shamir-wasm-host/src/wasm/wasm_function.rs:759; changed by: none
- R0 note (includes the fix site): OBSERVED: Function, procedure and scalar Err arms still call __rt::trap, and ordinary guest panic traps still map to Compute; the new typed fuel/interruption branches do not provide a user-error envelope. Fix site: crates/shamir-sdk-macros/src/lib.rs must encode deliberate errors in a transport decoded as User by the host.

### RV-SDK-MACROS-2638FAFF — `#[validator]` `shamir_call`: full decoded payload retained across the entire await + redundant deep copies of `record`/`old_record`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk-macros/src/lib.rs:142; crates/shamir-sdk-macros/src/lib.rs:157; changed by: none
- R0 note (includes the fix site): OBSERVED: Validator expansion still clones record and old_record out of borrowed Params and leaves the owning Params in scope through block_on; the malformed-parameter repair did not remove this redundant recursive ownership. Fix site: crates/shamir-sdk-macros/src/lib.rs should consume the decoded entries or release the original payload before invoking validation.

### RV-SDK-MACROS-966AA3AD — Raw function identifiers panic during helper-name construction

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-sdk-macros/src/lib.rs:55; crates/shamir-sdk-macros/src/lib.rs:547; changed by: none
- R0 note (includes the fix site): OBSERVED: All four emitters still interpolate the original identifier into Ident::new without removing a raw prefix. INFERRED: r#type still forms invalid __shamir_impl_r#type and panics during expansion. Fix site: crates/shamir-sdk-macros/src/lib.rs must normalize raw identifiers before helper-name construction.

### RV-SDK-MACROS-FB968C81 — All signature validation panics via `assert!`/`panic!` instead of `syn::Error::to_compile_error()`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-sdk-macros/src/lib.rs:58; crates/shamir-sdk-macros/src/lib.rs:204; changed by: none
- R0 note (includes the fix site): OBSERVED: Asyncness, arity and semantic return checks still use assert!/panic! rather than spanned syn errors; structural return matching changes which types pass, not how invalid signatures are reported. Fix site: crates/shamir-sdk-macros/src/lib.rs must return semantic validation errors through to_compile_error().

### RV-SDK-MACROS-0A76D271 — `unsafe` slice construction from unverified `(ptr, len)`; negative `len` is immediate UB

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-sdk-macros/src/lib.rs:130; crates/shamir-wasm-host/src/wasm/wasm_function.rs:610; changed by: none
- R0 note (includes the fix site): OBSERVED: Public safe shamir_call exports still construct slices from unchecked integer ptr/len; normal host invocation checks input length and guest memory bounds, but those checks do not constrain direct safe export calls. INFERRED: negative lengths or null pointers violate slice preconditions. Fix site: crates/shamir-sdk-macros/src/lib.rs must make the generated raw-pointer ABI contract sound, not merely add a sign check.

### RV-SDK-MACROS-D7F6AEC5 — User error strings passed verbatim into the trap channel

- R0 verdict: `unverified`, risk `low`; evidence: crates/shamir-sdk/src/runtime/__rt.rs:227; crates/shamir-wasm-host/src/wasm/wasm_function.rs:763; changed by: none
- R0 note (includes the fix site): OBSERVED: Error text still enters guest panic formatting and the host formats Wasmtime errors separately; generated guest profiles retain panic=abort without an explicit panic-message transport. The exact missing premise is whether the aborting guest's message reaches the returned Wasmtime error verbatim. Fix site: crates/shamir-sdk/src/runtime/__rt.rs is the guest message boundary to coordinate with an explicit host error-transport policy.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.

## Orchestrator constraints (binding)

- The guest↔host trap/error ABI must NOT change in this stage: transporting typed error categories across the WASM boundary is deferred to follow-up task "guest SDK error-category transport across the WASM boundary" (6D2497CA, decided under H9). For BB6378E2 / D7F6AEC5 fix what is possible inside the macro without an ABI change (no panic-trap for a user `Err`: route it through the existing user-error return path; sanitize/limit the user string before it enters the trap channel), and state explicitly in the final message anything that needs the ABI change.
- This branch is based on `fix/p1-sdk` (H9, not yet merged): do not modify `crates/shamir-sdk/src/**` beyond what a macro change strictly requires.
- Compile-error tests (FB968C81): use the existing compile-fail/trybuild harness if the crate has one; if not, add pure `syn`-level unit tests that call the expansion function and assert it returns `Err(syn::Error)` with the expected message — do not add a new dev-dependency without stating it.
