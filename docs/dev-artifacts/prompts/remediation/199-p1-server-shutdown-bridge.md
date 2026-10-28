# P1 stage G1b — p1-server-shutdown-bridge

## Goal

Graceful shutdown quiesces already accepted connection tasks before flushing, and the subscription bridge `close_all()` / `attach_handle()` race cannot leak a bridge task past connection teardown. Split from stage G1 (G1a merged as #319); only the two cards below are in scope.

## Cards

### RV-SERVER-FE6B4C98 — Graceful shutdown flushes without quiescing already accepted connection tasks

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-server/src/engine/server/server_launcher/helpers.rs:117; crates/shamir-server/src/engine/server/server_handle.rs:158; changed by: none
- R0 note (includes the fix site): OBSERVED: accepted connections receive cancellation, but cancellation drops request_loop before its writer-join teardown, and connection-drain timeout merely warns before final flush. Fix site: implement owned connection/writer quiescence and explicit timeout outcomes in crates/shamir-server/src/engine/server/server_launcher/helpers.rs and engine/server/server_handle.rs.

### RV-SERVER-6CA31DF8 — `close_all()` / `attach_handle()` race can leak a bridge task past connection teardown

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-server/src/engine/subscriptions/flow/registry.rs:134; crates/shamir-server/src/sync/connection/request_loop.rs:413; changed by: none
- R0 note (includes the fix site): OBSERVED: Teardown removes placeholders before aborting/draining dispatch; subscribe still reserves, spawns and attaches, and attach_handle has no missing-entry abort path. INFERRED: a concurrent dispatch can attach after removal and drop-detach its live bridge handle. Fix site: crates/shamir-server/src/engine/subscriptions/flow/registry.rs must cancel unattachable handles and coordinate admission with teardown.


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
