# P0 stage B1 — p0-server-auth

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Bootstrap-token TTL is checked at proof time; an unauthenticated connection is dropped after a bounded handshake deadline; TxBegin/TxExecute/TxCommit honour `NodeMode::ReadOnly`.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-SERVER-17329FC9 — Bootstrap-token TTL check uses authentication-start time after awaiting proof

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-server/src/sync/connection/handshake.rs:210; crates/shamir-server/src/sync/connection/handshake.rs:431
- Revalidation note: AGREE — OBSERVED: the expiry decision still passes the pre-challenge timestamp after awaiting proof, and metadata expiry compares its deadline against that supplied timestamp rather than fresh grant-time time.

### RV-TRANSPORT-TCP-B5244FA2 — Production server can retain an unauthenticated connection indefinitely after sending challenge

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-server/src/sync/connection/handshake.rs:291; crates/shamir-server/src/sync/connection/handshake.rs:771
- Revalidation note: AGREE: the proof read and enclosing SCRAM call remain deadline-free after the first-frame timeout, while accepted-task shutdown cancellation supplies termination only when shutdown occurs.

### RV-SERVER-C73466A7 — Interactive-tx path bypasses the read-only-replica gate entirely

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-server/src/engine/db_handler/handler.rs:614; crates/shamir-server/src/engine/db_handler/repo_admin/tx_handlers.rs:198
- Revalidation note: AGREE — OBSERVED: despite the visitor commit's broader descriptive claim, current TxBegin, TxExecute and TxCommit contain no NodeMode guard, while Execute alone rejects handler-configured read-only writes.

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
