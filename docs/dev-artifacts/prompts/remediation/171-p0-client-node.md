# P0 stage C3 — p0-client-node

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): JS calls accept a timeout / abort signal; `host` accepts hostnames.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-CLIENT-NODE-1CFB3796 — No timeouts surfaced and no cancellation: JS callers get the core SDK's known unbounded-hang class as the *only* behavior

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-client-node/src/lib.rs:132; crates/shamir-client/src/client/connect.rs:94
- Revalidation note: AGREE — OBSERVED: napi still supplies None for both timeouts and exposes no cancellation option, while TLS and SCRAM awaits lack a complete-handshake deadline.

### RV-CLIENT-NODE-8FB7D9BF — Documented hostname usage can never connect: `host` is parsed as an IP literal

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-client-node/src/lib.rs:53; crates/shamir-client-node/src/lib.rs:100
- Revalidation note: AGREE — OBSERVED: the public hostname input is still concatenated with its port and parsed as SocketAddr, with neither DNS resolution nor unbracketed IPv6 construction.

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
