# P0 stage B2 — p0-connect-session

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Audit chain memory is bounded; the refill watermark uses `fetch_max`; the public dispatch applies the post-auth limit; resume respects `MAX_SESSIONS_PER_USER`; the TOFU pin fires only after identity verification; no lock is held across an all-sessions scan; persistence failure is distinguished from replay; the `client` feature builds alone.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-CONNECT-8A65768F — AuditChain accumulates every audit event in an ever-growing in-memory Vec

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-connect/src/server/infra/audit_chain.rs:214; crates/shamir-connect/src/server/infra/audit_chain.rs:417
- Revalidation note: AGREE — OBSERVED: every append still clones into an unbounded retained Vec, and the production durable writer delegates to that same retaining chain before writing its appender.

### RV-CONNECT-06AB39CF — Rate-limiter refill watermark can regress — the exact #1090 hazard, unfixed at the sibling site

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/limits/rate_limit.rs:342; crates/shamir-server/src/sync/connection/handshake.rs:680
- Revalidation note: AGREE — OBSERVED: refill still assigns the supplied timestamp unconditionally, and production captures it before map acquisition, permitting stale concurrent calls to regress the watermark and recredit an interval.

### RV-CONNECT-8AD46452 — Public dispatch_request silently skips the post-auth rate limit

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/infra/dispatch.rs:100; crates/shamir-connect/src/server/infra/dispatch.rs:153
- Revalidation note: AGREE — OBSERVED: public owned dispatch still reaches the handler without consuming a rate token, whereas borrowed-view dispatch enforces the bucket and remains the production request-loop path.

### RV-CONNECT-8AD8B1C8 — Resume path bypasses MAX_SESSIONS_PER_USER

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/state/resume.rs:432; crates/shamir-server/src/sync/connection/handshake.rs:580
- Revalidation note: AGREE — OBSERVED: resume still inserts without the per-user cap while full authentication alone enforces it, leaving independent valid ticket families able to exceed sixteen stored sessions.

### RV-CONNECT-0FC58D93 — TOFU pin callback fires before Ed25519 identity verification completes

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/client/handshake.rs:274; crates/shamir-connect/src/client/handshake.rs:288
- Revalidation note: AGREE — OBSERVED: TOFU still invokes the caller callback after SCRAM verification but before identity-signature verification, allowing persistent callback effects on a subsequently rejected handshake.

### RV-CONNECT-D8C2825A — SessionStore::cap_lock: unjustified parking_lot::Mutex held across an O(all-sessions) scan

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/state/session.rs:470; crates/shamir-server/src/sync/connection/handshake.rs:580
- Revalidation note: AGREE — OBSERVED: successful full authentication still enters the global cap mutex and traverses every session before sorting the matching user's entries.

### RV-CONNECT-ECE3E157 — FjallConsumedCounters::try_advance conflates persistence failure with replay and logs nothing

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/infra/durable_counters.rs:126; crates/shamir-connect/src/server/state/resume.rs:366
- Revalidation note: AGREE — OBSERVED: counter get, insert and persist failures still collapse into false without connect-layer diagnostics and become the same AuthFailed result as replay rejection.

### RV-CONNECT-FED6910E — client feature cannot build without server

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/lib.rs:29; crates/shamir-connect/src/client/bootstrap.rs:17
- Revalidation note: AGREE — OBSERVED: client-only remains documented and enables no server feature, while bootstrap and password-change modules still unconditionally import feature-gated server types.

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
