# P1 stage H4 — p1-client-node

## Goal

Node binding: buffer isolation, demux mutex, close ordering. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H4): JS Buffers are copied or locked before async use; the demultiplexed client does not serialize requests behind one mutex; `close()` ordering cannot race in-flight calls.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-CLIENT-NODE-067017C8 — Caller-owned Buffer aliases are read on async workers without mutation isolation

- R0 verdict: `confirmed-open`, risk `high`; evidence: crates/shamir-client-node/src/lib.rs:118; crates/shamir-client-node/src/lib.rs:269; changed by: none
- R0 note (includes the fix site): OBSERVED: trustedPin copying and repl Buffer deserialization remain inside async napi functions, while the JS wrapper forwards the original repl Buffer and napi remains pinned to 3.10.5; no synchronous ownership snapshot was added. Fix site: shamir-client-node/src/lib.rs must establish owned input synchronously before worker dispatch for both inputs.

### RV-CLIENT-NODE-105A725D — One `tokio::sync::Mutex` held across every request serializes the demultiplexed client

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client-node/src/lib.rs:214; crates/shamir-client-node/src/lib.rs:243; changed by: none
- R0 note (includes the fix site): OBSERVED: ping and execute still retain the exclusive binding mutex through the core response await; optional deadlines bound waiting but do not permit concurrent registration. Fix site: shamir-client-node/src/lib.rs must release binding-state exclusivity before awaiting core requests.

### RV-CLIENT-NODE-F90A248F — Concurrent JS callers see stale pin/ticket snapshots only (correct), but `close()`'s take-then-await ordering can strand in-flight callers behind a permanent lock (see 4.1/6.1)

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-client-node/src/lib.rs:111; crates/shamir-client-node/src/lib.rs:378; changed by: 60b856bd (#300)
- R0 note (includes the fix site): OBSERVED: configured request deadlines now bound requests and close, but omission preserves unbounded waits; an unanswered request still owns the mutex close needs, and close retains its guard across shutdown. Fix site: shamir-client-node/src/lib.rs must let close initiate core shutdown independently of the request-held binding mutex.

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
