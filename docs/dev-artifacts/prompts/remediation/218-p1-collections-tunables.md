# P1 stage I2 — p1-collections-tunables

## Goal

Collections docs/fallible capacity, tunables validation. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row I2): Collections API documented with fallible capacity constructors; tunables setters validate and do not truncate to zero; visibility/override tests added.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-COLLECTIONS-201D84D3 — Public API mostly undocumented; `_wc` naming cryptic; doctests disabled

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-collections/src/lib.rs:21; crates/shamir-collections/src/lib.rs:55; changed by: none
- R0 note (includes the fix site): OBSERVED: expanded TMap/TSet contract documentation does not document THasher or the eight constructors; disabled doctests remain policy, not an independent defect. Fix site: shamir-collections/src/lib.rs needs rustdoc for the remaining public aliases/helpers, including capacity semantics.

### RV-COLLECTIONS-A5293F80 — Infallible capacity constructors can only abort the process; no fallible counterpart exists

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-types/src/codecs/interned/messagepack.rs:324; crates/shamir-collections/src/lib.rs:59; changed by: df72cadc (#283)
- R0 note (includes the fix site): OBSERVED: raw map decoding now bounds initial capacity by remaining bytes, closing that caller's header-only reservation trigger; collection capacity helpers remain undocumented and infallible, and the separate de-intern caller still trusts header counts. Fix site: shamir-collections/src/lib.rs needs explicit allocation-failure contracts or fallible helpers coordinated with untrusted-count consumers; the literal only-abort title remains overbroad.

### RV-TUNABLES-C09F4AA6 — Setters accept degenerate values unvalidated; millisecond quantization silently truncates to zero

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-tunables/src/runtime.rs:60; crates/shamir-server/src/sync/connection/request_loop.rs:153; changed by: none
- R0 note (includes the fix site): OBSERVED: as_millis() as u64 still truncates 900 microseconds to zero and wraps exactly 2^64 milliseconds to zero without documented precision/range policy; the server independently clamps in-flight capacity to at least one. Fix site: crates/shamir-tunables/src/runtime.rs must define and enforce the interval conversion policy.

### RV-TUNABLES-30F8ABA8 — TDD coverage gaps on the runtime surface: degenerate inputs, truncation, and override-effect never tested

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-tunables/src/tests/runtime_tests.rs:35; crates/shamir-tunables/src/tests/env_tests.rs:10; changed by: none
- R0 note (includes the fix site): OBSERVED: runtime tests still exercise ordinary exact values only; the newly registered tests cover environment parsing, not runtime duration truncation, overflow, degenerate setters, or runtime consumer effects. Fix site: crates/shamir-tunables/src/tests/runtime_tests.rs needs distinguishing conversion-boundary oracles; consumer-effect coverage remains conditional on runtime wiring.

### RV-TUNABLES-363B3879 — No test pins cross-thread visibility of an override; `reads_are_shared_ref` runs single-threaded

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-tunables/src/tests/runtime_tests.rs:52; crates/shamir-tunables/src/runtime.rs:18; changed by: none
- R0 note (includes the fix site): OBSERVED: the Arc clone, override and assertion all remain on one thread, and the current tunables tests contain no spawned-thread sharing oracle. Fix site: crates/shamir-tunables/src/tests/runtime_tests.rs needs a synchronized cross-thread override/read check, without claiming immediate unsynchronized freshness.

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
