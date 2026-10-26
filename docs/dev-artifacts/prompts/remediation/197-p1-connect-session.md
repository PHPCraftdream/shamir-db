# P1 stage G3 — p1-connect-session

## Goal

Rate limiter, resume keys, rotation CAS, zeroize, wire errors. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G3): `with_rate(_, 0)` is rejected or well-defined; resume key rotation invalidates cipher caches; handler errors are mapped before the wire; push data is serde_bytes; identity rotation is a CAS; password buffers are zeroized on every return; clock regressions cannot underflow; lock-site comments and doc fixes applied.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-CONNECT-064F9FE7 — InMemoryRateLimiter::with_rate(_, 0) breaks the limiter after warmup

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/limits/rate_limit.rs:203; crates/shamir-connect/src/server/limits/rate_limit.rs:369; changed by: none
- R0 note (includes the fix site): OBSERVED: both explicit-rate constructors still accept zero; after warmup effective rate is zero, reaching division by zero for an existing bucket or capacity-minus-cost for a new bucket. Fix site: server/limits/rate_limit.rs must reject or explicitly define zero-rate construction; the refill-watermark fix does not guard this trigger.

### RV-CONNECT-B11F48CA — ResumeConfig public key mutation leaves permanent cipher caches bound to old keys

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/server/state/resume.rs:147; crates/shamir-connect/src/server/state/resume.rs:181; changed by: none
- R0 note (includes the fix site): OBSERVED: raw ticket keys remain public and mutable while ciphers() permanently initializes OnceLocks; process_resume consumes those caches, so mutation after warming still leaves old cipher state active. Fix site: server/state/resume.rs must enforce key immutability or invalidate/rebuild caches through controlled mutation; shipped immutable shared configuration limits the scope.

### RV-CONNECT-2A423D73 — RequestHandler::handle Err(String) flows verbatim onto the wire

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/server/infra/dispatch.rs:180; crates/shamir-server/src/engine/db_handler/handler.rs:525; changed by: none
- R0 note (includes the fix site): OBSERVED: handler Err strings still enter ErrorEnvelope unchanged after the current admission guards; production response-encoding diagnostics remain a concrete source, although request-decode failures use structured DB responses. Fix site: server/infra/dispatch.rs needs an explicit safe application-error boundary rather than forwarding arbitrary handler diagnostics.

### RV-CONNECT-60D4DAB1 — PushEnvelope.data is not serde_bytes-wrapped

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/common/messages/push_envelope.rs:32; crates/shamir-server/src/engine/subscriptions/flow/push.rs:29; changed by: none
- R0 note (includes the fix site): OBSERVED: both owning Vec and production borrowed slice payloads remain unannotated byte sequences, with only Option omission attributes and no serde_bytes adapter. Fix site: common/messages/push_envelope.rs and the production PushEnvelopeRef in subscriptions/flow/push.rs need a coordinated binary-wire encoding decision.

### RV-CONNECT-F8A5E821 — ServerIdentityState::rotate is a non-atomic check-then-store on ArcSwap

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/server/ops/rotation.rs:151; crates/shamir-connect/src/server/ops/rotation.rs:184; changed by: none
- R0 note (includes the fix site): OBSERVED: rotate and finalization still load snapshots and publish with unconditional stores; the version mirror is published separately. Concurrent library callers can lose updates; no production rotate caller was found. Fix site: crates/shamir-connect/src/server/ops/rotation.rs, coordinate snapshot transitions and version publication.

### RV-CONNECT-226DEDF4 — Password buffers are not zeroized on early-error returns, violating the documented contract

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/client/handshake.rs:209; crates/shamir-connect/src/client/changepw.rs:43; changed by: none
- R0 note (includes the fix site): OBSERVED: validation and derivation errors still return before explicit password wipes, with no scope-exit guard; bootstrap retains the same ordering. Fix site: client/handshake.rs and client/changepw.rs need all-exit wiping for borrowed password buffers, with the same treatment in client/bootstrap.rs.

### RV-CONNECT-C9066D95 — Subtraction underflow on backwards clock in changePassword TTL check

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/server/auth/changepw.rs:130; crates/shamir-server/src/engine/db_handler/handlers/admin.rs:518; changed by: none
- R0 note (includes the fix site): OBSERVED: the live caller supplies wall-clock time and the verifier consumes the challenge before unchecked unsigned subtraction; no backwards-time guard precedes it. Fix site: crates/shamir-connect/src/server/auth/changepw.rs, reject reversed timestamps without underflow.

### RV-CONNECT-EAB07B19 — FjallConsumedCounters doc claims lock is not held across fsync

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-connect/src/server/infra/durable_counters.rs:60; crates/shamir-connect/src/server/infra/durable_counters.rs:231; changed by: none
- R0 note (includes the fix site): OBSERVED: the comment still says the lock is not held across fsync, but the guard acquired at line 175 remains alive through persist(SyncAll). Fix site: crates/shamir-connect/src/server/infra/durable_counters.rs, correct the comment while preserving serialized acceptance and persistence.

### RV-CONNECT-94E37810 — Error::to_wire is unused and its collapse set untested

- R0 verdict: `partially-fixed`, risk `nit`; evidence: crates/shamir-connect/src/common/model/error.rs:110; crates/shamir-connect/src/server/tests/session_flow/resume_cap_tests.rs:155; changed by: 72ba6527 (#284)
- R0 note (includes the fix site): OBSERVED: a new direct assertion checks AdmissionRejected collapses to AuthFailed, refuting the literal definition-only/no-assertion claim; no production helper call or complete collapse-set oracle was found. Fix site: common/model/error.rs and its tests need an exhaustive vocabulary oracle and an explicit decision whether production boundaries should use the helper.

### RV-CONNECT-34144ECE — Lock sites missing inline contention-model comments

- R0 verdict: `partially-fixed`, risk `nit`; evidence: crates/shamir-connect/src/server/state/session.rs:431; crates/shamir-connect/src/server/infra/audit_chain.rs:132; changed by: 72ba6527 (#284)
- R0 note (includes the fix site): OBSERVED: the session cap mutex was removed and its replacement indexes document contention and lock order; audit inner, reference-directory next_id and bootstrap inner still lack the requested model, while the semaphore documents blocking behavior. Fix site: server/infra/audit_chain.rs remains the repeated-path documentation gap, with the distinct rare/reference sites in server/ops/admin.rs and server/auth/bootstrap.rs.

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
