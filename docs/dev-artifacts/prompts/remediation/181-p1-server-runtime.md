# P1 stage G1 — p1-server-runtime

## Goal

Server response budget, shutdown quiesce, follower backoff, secrets. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G1): The response-budget guard lives until writer handoff; graceful shutdown quiesces accepted connections before the flush; `close_all`/`attach_handle` cannot leak a bridge task; follower backs off on unexpected Hello; the replicator password is a secret type; wire version types agree; `safe_run` and `/readyz` not-ready are exercised; `/info` docs match.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-SERVER-FB3EC046 — Production response-budget guard is destroyed before writer handoff

- R0 verdict: `confirmed-open`, risk `high`; evidence: crates/shamir-server/src/sync/connection/request_loop.rs:340; crates/shamir-server/src/security/byte_budget.rs:339; changed by: none
- R0 note (includes the fix site): OBSERVED: dispatch completes inside the task-local scope, but guard extraction remains outside it, while execute still stashes the reservation; INFERRED: the scoped guard is released before writer ownership. Fix site: extract or return the guard inside the scope in crates/shamir-server/src/sync/connection/request_loop.rs.

### RV-SERVER-FE6B4C98 — Graceful shutdown flushes without quiescing already accepted connection tasks

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-server/src/engine/server/server_launcher/helpers.rs:117; crates/shamir-server/src/engine/server/server_handle.rs:158; changed by: none
- R0 note (includes the fix site): OBSERVED: accepted connections receive cancellation, but cancellation drops request_loop before its writer-join teardown, and connection-drain timeout merely warns before final flush. Fix site: implement owned connection/writer quiescence and explicit timeout outcomes in crates/shamir-server/src/engine/server/server_launcher/helpers.rs and engine/server/server_handle.rs.

### RV-SERVER-6CA31DF8 — `close_all()` / `attach_handle()` race can leak a bridge task past connection teardown

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-server/src/engine/subscriptions/flow/registry.rs:134; crates/shamir-server/src/sync/connection/request_loop.rs:413; changed by: none
- R0 note (includes the fix site): OBSERVED: Teardown removes placeholders before aborting/draining dispatch; subscribe still reserves, spawns and attaches, and attach_handle has no missing-entry abort path. INFERRED: a concurrent dispatch can attach after removal and drop-detach its live bridge handle. Fix site: crates/shamir-server/src/engine/subscriptions/flow/registry.rs must cancel unattachable handles and coordinate admission with teardown.

### RV-SERVER-D3B174F4 — Follower loop busy-loops on an unexpected `Hello` reply (missing backoff)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-server/src/sync/replication/follower_loop.rs:716; crates/shamir-server/src/sync/replication/identity_fence/checks.rs:169; changed by: none
- R0 note (includes the fix site): OBSERVED: the identity fence accepts non-Pull variants, and a non-regressing Hello reaches immediate continue after backoff reset; INFERRED: repeated malformed upstream replies permit latency-limited rapid retries. Fix site: reject or back off unexpected Hello replies in crates/shamir-server/src/sync/replication/follower_loop.rs.

### RV-SERVER-EBC0829D — `ReplicationConfig::replicator_password` is a plain `String` held for the server's lifetime, reachable via `Debug`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-server/src/boot/config.rs:132; crates/shamir-server/src/sync/replication/transport/prod_factory.rs:47; changed by: none
- R0 note (includes the fix site): OBSERVED: ReplicationConfig derives Debug over Option<String>, and reconnect credentials retain ordinary Arc<str>; the intervening factory change only adds handshake_timeout. Fix site: redact and protect retained secrets in crates/shamir-server/src/boot/config.rs and sync/replication/transport/prod_factory.rs.

### RV-SERVER-9B8FC4F8 — `server_query_version` wire field is `u8` but the version constant is `u32`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-server/src/sync/connection/handshake.rs:671; crates/shamir-server/src/sync/connection/session/resume.rs:108; changed by: none
- R0 note (includes the fix site): OBSERVED: both authentication routes still cast the u32 version constant to u8, and client storage remains AtomicU8; version 4 fits, so truncation is prospective rather than a current failure. Fix site: coordinate checked conversion or wire/client widening starting in crates/shamir-server/src/sync/connection/handshake.rs and session/resume.rs.

### RV-SERVER-024B9BC4 — `safe_run`'s panic-survival contract is never actually exercised by a panicking tick

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-server/src/boot/scheduler.rs:314; crates/shamir-server/tests/misc/runtime/scheduler.rs:46; changed by: none
- R0 note (includes the fix site): OBSERVED: Production still catches unwinding ticks, but all scheduler fixture callbacks only count calls; the changed counter-store return type added no panic-then-success oracle. Fix site: crates/shamir-server/tests/misc/runtime/scheduler.rs needs a first-tick unwind followed by an observed successful later tick.

### RV-SERVER-4046C528 — `/readyz` is never observed returning "not ready" through the real boot path

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-server/src/security/observability/mod.rs:582; crates/shamir-server/tests/misc/audit_log/observability_http.rs:149; changed by: none
- R0 note (includes the fix site): OBSERVED: The false readiness state still returns 503, but registered readiness HTTP coverage only checks 200 after launch; the launcher binds data listeners before spawning observability and marking ready. Fix site: crates/shamir-server/tests/misc/audit_log/observability_http.rs should directly spawn observability before mark_ready and assert the 503-to-200 transition rather than assume a pre-listener boot window.

### RV-SERVER-5ED48F45 — `/info` endpoint documented as human-readable but returns raw msgpack

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-server/src/security/observability/mod.rs:26; crates/shamir-server/src/security/observability/mod.rs:632; changed by: none
- R0 note (includes the fix site): OBSERVED: Module prose still promises pretty-printed operator output, while info_handler serializes named MessagePack with application/msgpack and the HTTP assertion decodes that format. Fix site: crates/shamir-server/src/security/observability/mod.rs must align its /info description with the existing binary wire contract.

### RV-SERVER-72CEAF9E — `use shamir_query_types::hmac as canon;` repeated 4x mid-function instead of hoisted to file top

- R0 verdict: `partially-fixed`, risk `nit`; evidence: crates/shamir-server/src/engine/db_handler/handlers/admin.rs:121; crates/shamir-server/src/engine/db_handler/handlers/admin.rs:377; changed by: none
- R0 note (includes the fix site): OBSERVED: the header import exists, but three unconditional function-local copies remain without an applicable exception. Fix site: hoist/remove the redundant local imports in crates/shamir-server/src/engine/db_handler/handlers/admin.rs.

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
