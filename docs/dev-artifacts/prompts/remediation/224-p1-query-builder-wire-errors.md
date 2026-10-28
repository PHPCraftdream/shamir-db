# P1 stage H8b — p1-query-builder-wire-errors

## Goal

Split from H8 (merged as #341). Finish card RV-QUERY-BUILDER-6931DBDE and add the revert-sensitive regressions H8 left out. Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the whole card). H8 already landed: fallible `to_request_via_msgpack()` (returns `Result`), macro crate paths, `SecretString` password, one-pass `rows_as`, macro grammar fixes. Do not redo those; read the current code first.

## Cards

### RV-QUERY-BUILDER-6931DBDE — Public API leaks `rmp_serde` error types; decode errors are re-labeled as encode errors

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder/src/wire/mod.rs:33; crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:35; changed by: none
- R0 note (includes the fix site): OBSERVED: ToWire still exposes rmp_serde encode errors and maps decode failures into encode::Error::Syntax; ResponseError publicly embeds decode::Error. Batch's new BuildError return does not change these APIs. Fix sites: crates/shamir-query-builder/src/wire/mod.rs and crates/shamir-query-builder/src/wire/response/batch_response_ext.rs, preserving codec phase behind builder-owned errors.

## Remaining work (binding)

1. **6931DBDE.** `crates/shamir-query-builder/src/wire/error.rs`: `WireError::Encode`/`Decode` must not expose `rmp_serde` error types in the public API. Store a builder-owned source (own error type or message) while keeping the codec phase (encode vs decode) correct. Decode failures must be reported as decode, not encode. Tests: a decoder-depth/malformed-input failure is a `Decode` error; encode failures stay `Encode` (`to_request_via_msgpack_tests.rs`, `wire_tests.rs`).
2. **Revert-sensitive regressions for H8 cards** (each must fail if the H8 fix is reverted; name it with the card hex suffix in lower case):
   - FC88A91A: the password is wrapped in `SecretString` as soon as it enters the builder, not at `build()` (`access_ddl_tests.rs` around :331).
   - C6841ED3: a consumer-style macro test that uses only `shamir_query_builder` paths, with no direct `shamir_collections`/`shamir_query_types` imports (`subscribe_macro_tests.rs` around :26).
   - 028443E5: `rows_as` performs one codec setup per call, not per row, observed through a counted `cfg(test)` seam (`response_tests.rs` around :167).
   - EEE42189: accumulating a builder chain in a loop does not re-interpolate a growing prefix (macros `lowering_tests.rs` around :37), observed structurally (token count or seam), not by timing.

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
