# P0 stage B6 — p0-transports

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): A cancelled Windows accept leaves the listener usable; a failed `set_permissions` removes the socket; native WS acceptance enforces the Origin policy; endpoint paths come from shared constants; `accept_browser_ws` has integration tests.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-TRANSPORT-IPC-EFF55124 — Cancelling Windows accept poisons a reusable listener without any I/O error

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-transport-ipc/src/windows.rs:75
- Revalidation note: AGREE: accept takes the pending instance before connect awaits and restores nothing on cancellation, so reuse reaches expect with None; the private-dir extraction leaves this ownership sequence unchanged.

### RV-TRANSPORT-IPC-3C21F8FA — Unix `bind()`: `set_permissions` failure leaks the socket file — with default (wide) permissions — and poisons the next start

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-ipc/src/unix.rs:50; crates/shamir-transport-ipc/src/unix.rs:73
- Revalidation note: AGREE — OBSERVED: permission failure still returns before the pathname-owning listener is constructed, leaving its unlinking Drop unavailable; the private-directory change does not modify this Unix path.

### RV-TRANSPORT-WS-F54426DC — Native WS acceptance ignores supplied Origin despite the endpoint policy

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-ws/src/server.rs:85; docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:96
- Revalidation note: AGREE: the native callback validates only the path and accepts no Origin policy, so a supplied denied Origin still bypasses the documented endpoint check.

### RV-TRANSPORT-WS-F5B1F757 — Endpoint paths hardcoded as string literals; no shared constants; incompatible with the server's configurable `path`

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-server/src/boot/config.rs:926; crates/shamir-transport-ws/src/server.rs:87
- Revalidation note: AGREE: configuration accepts arbitrary slash-prefixed paths but launcher and accept-loop calls pass no configured path, leaving both acceptors bound to fixed literals.

### RV-TRANSPORT-WS-4FC6BCDD — `accept_browser_ws` — the Origin-enforcing handshake path — has zero integration tests; the framing length-mismatch invariant is also untested

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-ws/src/tests/server_tests.rs:7; crates/shamir-transport-ws/tests/framing_round_trip.rs:69
- Revalidation note: AGREE: transport upgrade tests still exercise only native acceptance and framing rejection still uses bare is_err, leaving the scoped browser rejection/status and malformed-prefix assertions absent.

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
