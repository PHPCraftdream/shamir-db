# P1 stage H1 — p1-client-transport

## Goal

Rust client cancel safety, bounded early buffer, typed errors. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H1): A cancelled request leaves no orphan pending entry and never desynchronises frames; `early_buffer` is bounded; demux locks justified or lock-free; no full-request clone per roundtrip; server error codes map to typed client errors.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-CLIENT-F91F11CE — Orphaned `pending` entries when the caller's future is cancelled (no drop guard)

- R0 verdict: `fixed` — CLOSED, change nothing for it. OBSERVED: registration immediately creates a guard whose Drop removes the pending entry on cancellation, including writer-wait and response-wait exits; a separate write-cancellation guard closes a partially written connection.

### RV-CLIENT-764D86D0 — Cancelling a partially written roundtrip leaves a live connection with corrupted frame alignment

- R0 verdict: `fixed` — CLOSED, change nothing for it. OBSERVED: The guard armed around the actual frame write shuts down the lifecycle when cancellation drops the future; subsequent requests check terminal state before registration and again after writer acquisition, preventing reuse of a partially written stream.

### RV-CLIENT-D5B9F8A3 — `early_buffer` key cardinality is server-controlled and unbounded for the life of the connection

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/client/transport.rs:509; crates/shamir-client/src/client/api.rs:139; changed by: none
- R0 note (includes the fix site): OBSERVED: Every unknown peer-supplied subscription ID still creates an early-buffer key with only a per-key envelope cap; registration removes only its matching key, and shutdown cleanup does not bound a live connection. Fix site: crates/shamir-client/src/client/transport.rs, aggregate early-buffer admission and retention limits.

### RV-CLIENT-B80C0D09 — `std::sync::Mutex` on the demux hot paths (`PendingMap`, `SubscriptionMap`, `EarlyBuffer`) without a sanctioned-category justification

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/client/transport.rs:161; crates/shamir-client/src/client/api.rs:425; changed by: none
- R0 note (includes the fix site): OBSERVED: Pending and combined subscription/early registries remain global standard mutexes on request and push paths; O(1) operations and no-await comments do not establish a sanctioned rare/setup-only category. Fix site: crates/shamir-client/src/client/transport.rs, lifecycle registry synchronization while preserving atomic close-and-register behavior.

### RV-CLIENT-E16A04E5 — `roundtrip` clones the whole serialized request per call and ignores the zero-copy envelope built for exactly this path

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/client/api.rs:409; crates/shamir-connect/src/common/messages/envelope.rs:92; changed by: none
- R0 note (includes the fix site): OBSERVED: roundtrip still clones serialized scratch and constructs RequestEnvelope::new despite the available borrowed RequestEnvelopeRef; output serialization remains necessary and performance impact is unmeasured. Fix site: crates/shamir-client/src/client/api.rs, request serialization and borrowed-envelope construction.

### RV-CLIENT-AE50D83D — Envelope-level server errors are stringly-typed into `ClientError::Protocol` — spec §14 codes are not machine-readable

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/client/transport.rs:549; crates/shamir-client/src/error.rs:23; changed by: none
- R0 note (includes the fix site): OBSERVED: Envelope errors still become formatted ClientError::Protocol strings rather than a structured server code; typed Db errors do not cover this demux branch. Fix sites: crates/shamir-client/src/client/transport.rs and error.rs, envelope-error representation and routing.

### RV-CLIENT-35A189A9 — Stringly-typed `Handshake`/`Tls`/`Transport`/`Protocol` variants erase typed sources

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/error.rs:11; crates/shamir-client/src/client/connect.rs:418; changed by: none
- R0 note (includes the fix site): OBSERVED: Tls, Transport, Handshake and Protocol still store strings, and process_auth_ok still stringifies its typed error; new close-cause categories do not restore these sources. Fix sites: crates/shamir-client/src/error.rs and client/connect.rs, error representation and conversions.

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
