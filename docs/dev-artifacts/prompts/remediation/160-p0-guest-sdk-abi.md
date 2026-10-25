# P0 stage C6 — p0-guest-sdk-abi

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Host-import buffers are freed on both sides; decode/encode failures surface as errors; validator params never coerce to Null; a Pending guest future is an explicit error; duplicate HTTP headers survive the codec; colon-prefixed keys round-trip.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-SDK-8190492D — Host-import ABI leaks both directions' buffers on every call — unbounded guest linear-memory growth in loops

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk/src/runtime/host_imports.rs:60; crates/shamir-sdk-macros/src/lib.rs:239
- Revalidation note: AGREE — OBSERVED: request encoding and generated response allocation still forget their Vec buffers, and response shims only borrow/decode those allocations without reclaiming them before invocation teardown.

### RV-SDK-A68C70DE — Fail-open protocol handling: decode/encode failures silently become plausible success values (7 decode sites + `decode_params` + `encode_value` + `Table::insert`'s message)

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk/src/runtime/host_imports.rs:183; crates/shamir-sdk/src/runtime/__rt.rs:11
- Revalidation note: AGREE — OBSERVED: response decoding retains absence/Null/empty-list fallbacks, parameter decoding retains empty Params, and encoding errors retain an empty-byte fallback.

### RV-SDK-MACROS-4DB0D871 — `#[validator]` silently coerces undecodable params into `record = Null`, `old_record = None`

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk/src/runtime/__rt.rs:11; crates/shamir-sdk-macros/src/lib.rs:129
- Revalidation note: AGREE — OBSERVED: malformed or non-map parameter bytes still become empty Params, after which validator extraction substitutes Null and None instead of rejecting the decoding failure.

### RV-SDK-MACROS-853FC350 — Return-type validation is string-based, per-macro inconsistent, and rejects valid spellings

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk-macros/src/lib.rs:197; crates/shamir-sdk-macros/src/lib.rs:411
- Revalidation note: AGREE — OBSERVED: function and validator retain restrictive spelling comparisons while procedure/scalar use different normalization that still omits std::result qualification.

### RV-SDK-MACROS-D44EF500 — The concurrency-critical "Ready on first poll" contract of the emitted ABI has zero test coverage in this crate

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk/src/tests/value_tests.rs:414; crates/shamir-sdk/src/runtime/__rt.rs:50
- Revalidation note: AGREE — OBSERVED: helper coverage exercises immediate readiness, and searches across SDK, macro and host sources found no controlled Pending/poll-count oracle distinguishing guest handling from host exhaustion.

### RV-SDK-MACROS-FEC0E5E4 — Guest executor has no suspension protocol for a genuinely Pending future

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk/src/runtime/__rt.rs:50; crates/shamir-wasm-host/src/wasm/wasm_function.rs:514
- Revalidation note: AGREE — OBSERVED: guest Pending immediately repolls with a no-op waker and no local bound, while host fuel and epoch limits constrain ordinary WASM execution rather than implementing guest suspension.

### RV-SDK-MACROS-D3AD2579 — Generated standard-library names remain caller-shadowable

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-sdk-macros/src/lib.rs:110; crates/shamir-sdk-macros/src/lib.rs:135
- Revalidation note: OBSERVED: Generated allocators retain bare Vec and vec names, and validator expansion retains bare Option, Some and None names that consumers can shadow.

### RV-SDK-MACROS-68D9A5D0 — Generic functions hit E0207 inside the expansion instead of a clear rejection

- Status at HEAD: `unverified`, risk `low`; evidence: crates/shamir-sdk-macros/src/lib.rs:219; crates/shamir-sdk-macros/src/lib.rs:264
- Revalidation note: OBSERVED: Expansion emits generic functions rather than an impl, so E0207 is not its mechanism; the missing premises remain the intended generic-support contract and exact compiler diagnostic for an unselected type parameter.

### RV-WASM-HOST-4C69CFE0 — HTTP wire codec collapses duplicate headers (`Set-Cookie` loss on both directions)

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk/src/api/http.rs:108; crates/shamir-wasm-host/src/wasm/bindings/http.rs:89
- Revalidation note: AGREE — OBSERVED: SDK requests still serialize duplicate header entries as a map that host QueryValue decoding collapses, and host response encoding still overwrites identically named headers despite supporting list-form requests.

### RV-WASM-HOST-B1A78B5B — Reserved colon-prefixed map keys violate guest-to-host value identity

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/types/value.rs:214; crates/shamir-wasm-host/src/wasm/bindings/call.rs:91
- Revalidation note: AGREE — OBSERVED: ordinary guest parameters still enter the String-key QueryValue visitor, which interprets reserved prefixes and inserts stripped names, so literal i:count keys can collide with count or reject noninteger values.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
