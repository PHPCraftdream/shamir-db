# P1 stage H3 — p1-client-auth-tests

## Goal

Client auth flags, password zeroize, negative-path tests. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H3): `auth_ok` flags reach the caller; SCRAM password buffers are zeroized; pin verification and resume have negative-path tests; vacuous tests replaced; untested public paths covered.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-CLIENT-F20C09BF — SDK silently discards `rotation_in_progress` and `kdf_upgrade_required` from `auth_ok` — orphan-recovery (spec §6.5) and KDF-upgrade hints (§13) are unreachable

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/client/connect.rs:395; crates/shamir-server/src/sync/connection/handshake.rs:663; changed by: none
- R0 note (includes the fix site): OBSERVED: client construction still substitutes None for both hints and the server's AuthOk construction omits them; process_auth_ok still rejects a changed pin before recovery handling. Fix site: client/connect.rs needs end-to-end hint propagation and an explicitly approved, strictly verified recovery path coordinated with server handshake serialization.

### RV-CLIENT-150F9230 — Cleartext password persists in unzeroized serialization buffers after `create_scram_user`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/client/api.rs:405; crates/shamir-transport-tcp/src/framing.rs:173; changed by: none
- R0 note (includes the fix site): OBSERVED: create_scram_user still reaches plain retained request scratch, its clone, owning envelope bytes and a plain framing Vec; SecretString destruction does not wipe these copies. Fix sites: crates/shamir-client/src/client/api.rs and crates/shamir-transport-tcp/src/framing.rs, secret-bearing serialization-buffer lifetimes.

### RV-CLIENT-2D0142DD — No negative-path coverage for pin verification or resume anywhere in the crate's suite

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-client/src/tests/client/resume_wire_tests.rs:233; crates/shamir-client/src/tests/interner/interner_cache_tests.rs:111; changed by: 24ac7a97 (#295)
- R0 note (includes the fix site): OBSERVED: Production resume_wire_exchange now has negative substituted-pin and bad-signature oracles asserting zero ticket disclosure; initial SCRAM connect fixtures remain unpinned TOFU and do not exercise trusted-pin rejection or denied TOFU. Fix site: crates/shamir-client/src/tests/interner/interner_cache_tests.rs or the client handshake test fixtures for initial-connect identity wiring.

### RV-CLIENT-E5D2A811 — Vacuous test: `atomic_u8_plumbing_stores_and_reads_correctly` tests std, not the client

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/tests/wire/wire_version_tests.rs:136; crates/shamir-client/src/tests/wire/v2_passthrough_tests.rs:157; changed by: none
- R0 note (includes the fix site): OBSERVED: the plumbing test constructs standalone AtomicU8 values, and the v2 roundtrip still returns early when the client reports an old version. Fix site: wire_version_tests.rs must exercise actual Client negotiation/getter plumbing instead of std atomics.

### RV-CLIENT-2D08F353 — TDD-coverage gaps: untested public paths

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-client/src/tests/client/lifecycle_tests.rs:471; crates/shamir-client/src/tests/client/resume_wire_tests.rs:345; changed by: 3084dfe8 (#286), 24ac7a97 (#295)
- R0 note (includes the fix site): OBSERVED: Public subscription closure, reader-abort liveness and production resume exchange now have substantive oracles; full Client::resume integration, successful public early-buffer flush and smart when/ForEach execution combinations remain uncovered by the inspected registrations and call sites. Fix sites: crates/shamir-client/src/tests/client/lifecycle_tests.rs, resume_wire_tests.rs and receipts/id_encoding_guard_tests.rs.

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
