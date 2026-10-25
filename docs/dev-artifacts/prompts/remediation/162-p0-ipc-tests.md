# P0 stage E-ipc — p0-ipc-tests

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): see the cards

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-TRANSPORT-IPC-07D10AFD — No test drives `accept()` through an error, and no Unix test pins bind-on-existing-path semantics

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-ipc/src/tests/unix_tests.rs:8; crates/shamir-transport-ipc/src/tests/windows_tests.rs:16
- Revalidation note: OBSERVED: Transport tests still cover successful accepts and ordinary Unix permission and cleanup behavior, not injected accept failures or stale/live Unix bind collisions.

### RV-TRANSPORT-IPC-466D0307 — Rotation leaves a no-pending-instance window → concurrent second client gets `ERROR_PIPE_BUSY`; the client does no BUSY retry

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-ipc/src/windows.rs:43; crates/shamir-transport-ipc/src/windows.rs:80
- Revalidation note: OBSERVED: Windows connect still performs one open and accept replenishes its sole pending instance only after connection, leaving the BUSY window without a client retry.

### RV-TRANSPORT-IPC-4E8BE4FD — No lifecycle tests for the Windows listener; Unix-only `Drop` coverage

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-ipc/src/tests/windows_tests.rs:17; crates/shamir-transport-ipc/src/tests/windows_tests.rs:42
- Revalidation note: OBSERVED: Both Windows tests exercise successful service, and neither releases all relevant handles and explicitly rebinds the same pipe name.

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
