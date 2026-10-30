# P34-04 - shamir-connect + shamir-server: wire-visible errors, fallible entropy/clock, KDF error, replication pins, bootstrap password

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Finish the fallible entropy/clock work (Ed25519 generation, Argon2 admission, session touch, identity-rotation update, remaining UnixNanos::now callers), keep ticket-version acceptance coherent with the authoritative rotation snapshot, close the remaining error-path test gaps, give the plain-profile loopback predicate a single owner, warn on the argv bootstrap password, document the replication-upstream pinning limit, and stop the long-poll replication source from serialising pull targets.

This is one change set for shamir-connect, shamir-server: 10 active findings, effort units 24 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-connect`, `shamir-server`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-client/src/tests/wire/wire_version_tests.rs` (8999F6B7)
- `crates/shamir-transport-tcp/src/listener.rs` (A65118CC)

## Acceptance

- Entropy and clock failure injection proves typed failure, cleanup and no partial publication on every migrated path (387EAA05).
- ServerIdentityState rotation publishes ticket-acceptance versions from the authoritative snapshot only (F8A5E821).
- Shared handshake byte fixtures exercise the real production DTOs of server and client (8999F6B7); the abrupt-termination durable-counter replay oracle and the old-derivation buffer-wipe case exist (F43EB41E).
- Boot warns once, without the value, when --bootstrap-password is used (97C240D7); the plain-profile loopback predicate has one owner and the accepted addresses are unchanged (A65118CC); the upstream-pinning limit is documented (E8E1D11B).
- Two pull targets of one WireReplSource: the second is not delayed by the first long poll (regression for the P2-U01 finding).

## Coordination with other tasks

Wave 1; no strict predecessor.
- 387EAA05 is the second tranche of the P2-10 decision; keep its rules (never substitute zero entropy or fabricated time, fail closed, no partial publication).
- Downstream shamir-server/shamir-client migrations for any API that becomes fallible are part of this stage; run `cargo check` of dependents.
- A65118CC reads shamir-transport-tcp listener.rs (ListenerProfile) and does not change its behaviour.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-CONNECT-387EAA05, RV-SERVER-E8E1D11B, RV-CONNECT-2A423D73, RV-CONNECT-7D41FB60, RV-CONNECT-E4631CD9, RV-SERVER-97C240D7, RV-SERVER-A65118CC. The full decision paragraph sits in the finding below.

- L3-RV-CONNECT-387EAA05: **IMPLEMENT** (change as decided).
- L3-RV-SERVER-E8E1D11B: **IMPLEMENT** (change as decided).
- L3-RV-CONNECT-2A423D73: **SKIP** (no change).
- L4-RV-CONNECT-7D41FB60: **SKIP** (no change).
- L4-RV-CONNECT-E4631CD9: **SKIP** (no change).
- L4-RV-SERVER-97C240D7: **IMPLEMENT** (change as decided).
- L4-RV-SERVER-A65118CC: **IMPLEMENT** (change as decided).

## Findings (ordered by risk, then size)

### L3-RV-CONNECT-387EAA05 - Library expect panics for OS RNG failure and pre-epoch clock

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `additive`, risk `medium`). Second tranche of the P2-10 decision; keep its rules: never substitute zero entropy or fabricated time, fail closed, and do no partial publication. (1) shamir-connect: add Ed25519Keypair::try_generate() -> Result<Self, EnvironmentError>, drawing its seed through try_random_bytes so the cfg(test) entropy-failure seam covers it, and wipe the seed. ServerIdentityState::rotate draws the candidate seed once, before the rcu, through that fallible path and returns Error::Environment on failure, leaving keys, rotation_until and version unchanged. (2) Argon2Semaphore::acquire_until reads the clock via UnixNanos::try_now; on clock failure it returns None (fail closed, the same outcome as a timeout and therefore server_busy). Add a private clock-injected core for tests. (3) shamir-server: the scheduler ticks (boot/scheduler.rs:138,155,172,191,231) use try_now; on failure they warn and skip that tick instead of panicking outside safe_run. sync/ports.rs:122,135,148 map a clock failure to PortError::other. repo_audit.rs:83 logs an error and skips that append. (4) Keep as documented legacy wrappers, each with a '# Panics' doc section and no #[deprecated] (a deprecation would force new #[allow]s that the rust-guard gate forbids): random_bytes, random_array, UnixNanos::now, Ed25519Keypair::generate, ServerIdentityState::fresh, make_bootstrap_challenge, build_hello, build_challenge_request, Session::touch, SessionStore::lookup, and the lockout/rate-limit snapshot(). Record these as the limitation: the persist_snapshot paths run inside scheduler safe_run, and default_node_id is a boot-time fail-fast. Compatibility: additive only; no signature, wire or persisted-format change. Regression tests rv_connect_387eaa05_*: rotate under with_entropy_failure returns Error::Environment and the identity is unchanged; acquire_until with a failing clock returns None without panicking; a scheduler/ports clock helper with a failing clock skips or returns an error without panicking.
- Evidence at `07bc804c`: crates/shamir-connect/src/common/primitives/crypto.rs:67; crates/shamir-connect/src/common/runtime/time.rs:18; crates/shamir-connect/src/server/state/session.rs:287; crates/shamir-connect/src/server/limits/argon2_semaphore.rs:97
- Recipe: Finish fallible entropy and clock propagation through remaining callers and resolve legacy panic-wrapper contracts.
- Verifier note: Typed alternatives exist, but public panic wrappers and concrete clock-panic callers remain; public compatibility needs a decision.

### L3-RV-SERVER-E8E1D11B - Replication client uses trust-on-first-use with no leader-key pinning

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Docs-only, accepting restart re-enrollment. Add an 'Upstream identity pinning' entry to docs/guide-docs/KNOWN_LIMITATIONS.md section 5 (Replication) and to the operational-limits paragraph of docs/guide-docs/guide/08-interconnect.md. It must say: (a) [replication].upstream_pins maps an upstream address (normalized; tcp://, tls:// and bare host:port are one key) to a list of 64-hex SHA256(leader Ed25519 public key) pins, and this is the only durable, verified enrollment; (b) without a configured pin, the first successful connect in each process is trusted (TOFU), logged at warn with a ready-to-paste enrollment line, and enforced for the rest of the process; learned pins are not persisted and are re-learned after every restart (an accepted limitation); (c) the leader logs its own identity pin at info on boot for out-of-band comparison; (d) for leader key rotation, add the new pin alongside the old one during the 7-day overlap, otherwise the subscription stops in state upstream_key_mismatch. Code, config schema, wire and persisted state are unchanged. Out of scope: persisting learned pins (that would make an unverified first observation durable and needs new persisted state plus rotation following). No test (docs-only).
- Evidence at `07bc804c`: crates/shamir-server/src/sync/replication/transport/prod_factory.rs:78-81; crates/shamir-server/src/sync/replication/transport/prod_factory.rs:115-128; crates/shamir-server/src/sync/replication/transport/prod_factory.rs:185-188
- Recipe: Define verified enrollment and rotation, then persist approved upstream pins or explicitly accept restart re-enrollment.
- Verifier note: Configured pins and process-local continuity are enforced before replication; learned pins restart empty, requiring enrollment and rotation policy.

### L3-RV-CONNECT-2A423D73 - RequestHandler::handle Err(String) flows verbatim onto the wire

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/-`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). The premise of the remaining facet is wrong. The handler Err(String) path is already collapsed to the fixed code application_error (crates/shamir-connect/src/server/infra/dispatch.rs:47-57,123). The remaining invalid_request message (crates/shamir-server/src/engine/db_handler/handler.rs:360-370) is a documented, tested client-facing contract: TRANSACTION_OUTCOMES.md:86-89 promises message: <detail>, invalid_request_tests.rs:1-2,113 pins the serde diagnostic, and the TS client test at client.test.ts:369-373 preserves detail. It describes only the authenticated caller's own request bytes against a public schema and exposes no server state. Record as an intended diagnostic contract.
- Evidence at `07bc804c`: crates/shamir-connect/src/server/infra/dispatch.rs:55; crates/shamir-connect/src/server/infra/dispatch.rs:123; crates/shamir-server/src/engine/db_handler/handler.rs:364
- Verifier note: First pass missed the card's production decoder-diagnostic facet: e.to_string remains wire-visible inside an Ok-serialized DbResponse::Error.

### L3-RV-CONNECT-F43EB41E - Consolidated error-path test gaps

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/test`.
- Evidence at `07bc804c`: crates/shamir-connect/src/server/tests/security/rate_limit_tests.rs:10; crates/shamir-connect/src/server/tests/infra_tests/durable_counters_tests.rs:63; crates/shamir-connect/src/server/tests/infra_tests/durable_counters_tests.rs:146; crates/shamir-connect/src/server/tests/session_flow/changepw_challenge_tests.rs:155; crates/shamir-connect/src/common/primitives/kdf_params.rs:34
- Recipe: Add old/new derivation-failure buffer-wipe cases and a CI abrupt-termination durable-counter replay oracle.
- Verifier note: Keep partial; refine recipe because new-password derivation failure is already covered, leaving old derivation and CI abrupt-termination replay coverage.

### L3-RV-CONNECT-F8A5E821 - ServerIdentityState::rotate is a non-atomic check-then-store on ArcSwap

- Verdict: `CHANGED`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-connect/src/server/ops/rotation.rs:100; crates/shamir-connect/src/server/ops/rotation.rs:167; crates/shamir-connect/src/server/ops/rotation.rs:202; crates/shamir-connect/src/server/ops/rotation.rs:223; crates/shamir-connect/src/server/state/resume.rs:335
- Recipe: Read ticket-acceptance versions from the authoritative ArcSwap snapshot and remove the separately published mirror.
- Verifier note: RCU removes lost snapshot updates; separate mirror publication still permits an inferred ticket-version coherence window.

### L4-RV-CONNECT-7D41FB60 - Auth maintenance bundle: stale references and inert suppressors

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Aggregate tracker, closed by reference, consistent with the P2-10 SKIP. Component dispositions: C8866EB1 fixed (auth_message.rs:5-6 now names test-vectors/). 850A9AFD fixed (no inert suppressors at server/auth/handshake.rs:232-245). E4631CD9 accepted (SKIP in this batch). 387EAA05 environmental panic debt tracked separately (IMPLEMENT in this batch, with a recorded legacy-wrapper limitation). Do not claim every component is repaired.
- Evidence at `07bc804c`: crates/shamir-connect/src/common/messages/auth_message.rs:6; crates/shamir-connect/src/server/auth/handshake.rs:238-245; crates/shamir-connect/src/common/primitives/kdf_params.rs:77-88
- Recipe: Resolve E4631CD9 once and close this aggregate by reference; keep environmental panic debt separate.
- Verifier note: Fixture reference and suppressors are corrected; resolve the public String KDF error through E4631CD9, keeping environmental failures separate.

### L4-RV-CONNECT-E4631CD9 - validate_client_kdf_safe returns String; its diagnostic is discarded

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted API debt, re-affirming the P2-10 SKIP. validate_client_kdf_safe (kdf_params.rs:77-91) has a single production caller, which discards the message and maps it to KdfParamsRejected (client/handshake.rs:214-217). That call is preceded by validate_client_limits, whose caps (256 MiB / 8, types.rs:123-125) are stricter than the helper's 512 MiB / 16, so the helper never rejects in the current handshake. Its direct tests depend on the memory/time diagnostics. Revisit only if the spec caps are relaxed.
- Evidence at `07bc804c`: crates/shamir-connect/src/common/primitives/kdf_params.rs:77-88; crates/shamir-connect/src/client/handshake.rs:214-216; crates/shamir-connect/src/common/model/types.rs:123-125
- Recipe: Replace the public String error with an agreed typed error and migrate callers and tests.
- Verifier note: Public String error API and diagnostic discard remain; stricter preceding caps prevent this helper's rejection in the current handshake.

### L4-RV-SERVER-8999F6B7 - Positional handshake DTOs independently mirrored with shamir-client

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/test`.
- Evidence at `07bc804c`: crates/shamir-server/src/tests/engine/auth_ok_hint_tests.rs:36-50; crates/shamir-server/src/tests/engine/auth_ok_hint_tests.rs:89-110; crates/shamir-client/src/tests/wire/wire_version_tests.rs:132-194
- Recipe: Consume shared base, hinted and resume byte fixtures in tests of both actual production DTOs.
- Verifier note: Exact server fixture still decodes a test mirror; shared base, hinted and resume fixtures should exercise both actual production DTOs.

### L4-RV-SERVER-97C240D7 - `bootstrap_password` accepted as a CLI argument

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/decision`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Keep --bootstrap-password (argv) for compatibility with existing scripts and the e2e harness. When it is used, emit one tracing::warn! at boot, without the value, saying argv may be visible in process lists and shell history and recommending --bootstrap-password-stdin or the default random-token file. Put the decision in a small pure helper next to bootstrap_mode (crates/shamir-server/src/main.rs:235-258). Update the user docs to teach the stdin form or the random-token default instead of argv: docs/guide-docs/guide/00-quickstart.md:15,21 and docs/guide-docs/guide/07-operations.md:42 (for example printf '%s\n' "$PW" | shamir-server --config ... --bootstrap-password-stdin), and adjust the flag mentions at IMPLEMENTATION_GUIDE.md:265 and TRANSPORT_UNIX.md:172 to list --bootstrap-password-stdin first. Existing argv, stdin, random-token and skip semantics, the conflicts and the help text stay unchanged. Regression test rv_server_97c240d7_*: the helper flags argv only, and the existing bootstrap_mode outcomes for argv/stdin/random/skip stay the same. Out of scope: removing or hard-deprecating argv, which is a breaking CLI change for a release decision.
- Evidence at `07bc804c`: crates/shamir-server/src/main.rs:54-68; crates/shamir-server/src/main.rs:449-463
- Recipe: Deprecate or remove plaintext argv input, or explicitly accept its compatibility and local-disclosure trade-off.
- Verifier note: Stdin alternative and warning exist, but plaintext argv remains a live bootstrap input; removal or acceptance is a compatibility-policy decision.

### L4-RV-SERVER-A65118CC - Duplicated normative loopback predicate with shamir-transport-tcp

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Make shamir-transport-tcp the single owner of the plain-profile loopback predicate. In crates/shamir-server/src/boot/config.rs:889, replace !is_loopback_ip(addr.ip()) with !shamir_transport_tcp::listener::ListenerProfile::Plain.allows(&addr) (existing public API, already used by the server launcher), and delete the server-local is_loopback_ip (config.rs:944-951) and its now-unused imports. The accepted address set is unchanged (127/8, ::1, IPv4-mapped 127/8; rejected: mapped non-loopback, unspecified, IPv4-compatible ::127.0.0.1) and the diagnostic text is unchanged. No new export, which keeps P2-18's 'do not export a new shared API'; this supersedes P2's 'keep separate helpers' for this one redundant copy. The existing P2 contract tests (crates/shamir-server/tests/admin/ops/config.rs) must pass unmodified, which proves no accepted address changed. Out of scope: the WS listener and any other predicates.
- Evidence at `07bc804c`: crates/shamir-server/src/boot/config.rs:889,945-950; crates/shamir-transport-tcp/src/listener.rs:40-55
- Recipe: Choose a shared predicate owner and route both validators through it without changing accepted addresses.
- Verifier note: Mapped-loopback behavior agrees, but independently maintained normative predicates remain; choose shared ownership without changing accepted addresses.

## New finding from the P2 sprint (P2-U01): WireReplSource long-poll lock

`WireReplSource` is shared by all pull targets of one subscription and its long polls hold the client lock for up to `poll_wait_ms` (default 5000), so N targets can wait on each other. The comments in `wire_source.rs` and `prod_factory.rs` that call it "single-tasked/uncontended" are stale. Fix it so a long poll of one target does not block the others (for example one client per target or a lock-free request multiplexer), correct the comments, and add a two-target regression test.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-connect -p shamir-server -- --check`; `cargo clippy -p shamir-connect -p shamir-server --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
