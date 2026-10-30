# P34-05 - shamir-client (+node binding) + transports: typed sources, resume ticket lifetime, framing reuse, tls tests, docs

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Preserve typed sources for envelope encoding failures, drop the redundant interner sync where it is provably unnecessary, reuse request framing storage on the client hot path, add the missing TLS exporter and write_frame_prereserved contract tests, qualify the exporter RFC wording, and replace guessed performance claims with CI measurements.

This is one change set for shamir-client, shamir-client-node, shamir-transport-tcp, shamir-transport-ipc: 13 active findings, effort units 23 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-client`, `shamir-client-node`, `shamir-transport-tcp`, `shamir-transport-ipc`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-connect/src/common/messages/envelope.rs` (35A189A9)

## Acceptance

- Envelope encoding errors keep their source chain (35A189A9).
- write_frame_prereserved reports a mismatched prefix as a malformed-buffer error before the size cap (884DDBF5); a real uncompleted rustls connection exporter case asserts its specific cause (A38C5274).
- Framing reuse allocates no second frame buffer on the roundtrip path and keeps write serialisation, terminal cancellation and zeroization of secret scratch (79910BA5), with an allocation oracle that fails on the old code.
- 66E9A5FC and 31C5E8A6: the measurement harness or oracle exists and its CI result is recorded; numbers are not guessed.
- B80C0D09 is skipped until the owner answers.

## Coordination with other tasks

Wave 2; starts after P34-04 are merged. P34-04 changes shamir-connect APIs and handshake fixtures that the client consumes (35A189A9 edits connect/common/messages/envelope.rs; 8999F6B7 edits client wire tests).
- 35A189A9 edits shamir-connect/src/common/messages/envelope.rs (a P34-04 crate): keep that edit minimal and run `cargo check -p shamir-connect`.
- 79910BA5 changes the client hot path and shamir-transport-tcp framing together; keep zeroization and terminal cancellation.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-CLIENT-35A189A9, RV-CLIENT-8997B366, RV-CLIENT-B80C0D09, RV-TRANSPORT-IPC-FA481516, RV-CLIENT-NODE-31D3BBE5. The full decision paragraph sits in the finding below.

- L3-RV-CLIENT-35A189A9: **IMPLEMENT** (change as decided).
- L3-RV-CLIENT-8997B366: **SKIP** (no change).
- L3-RV-CLIENT-B80C0D09: **ESCALATE** (waits for the owner).
- L3-RV-TRANSPORT-IPC-FA481516: **SKIP** (no change).
- L4-RV-CLIENT-NODE-31D3BBE5: **SKIP** (no change).

## Findings (ordered by risk, then size)

### L3-RV-CLIENT-35A189A9 - Stringly-typed `Handshake`/`Tls`/`Transport`/`Protocol` variants erase typed sources

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). In crates/shamir-client/src/client/api.rs (roundtrip REQ_BUF closure), stop calling RequestEnvelopeRef::to_msgpack and stop mapping its failure to ClientError::Protocol(format!("envelope encode: {e}")). Instead encode the borrowed RequestEnvelopeRef with rmp_serde::encode::write_named into a Zeroizing<Vec<u8>> pre-sized to buf.len()+64, mapping failure to the existing source-bearing ClientError::Encode(rmp_serde::encode::Error). Put this in a private generic helper over T: Serialize so it can be tested; it can live in api.rs because the client/ folder already has 7 entries. Partial output on failure must be wiped, both the existing scratch zeroize and the Zeroizing output. Must stay compatible: wire bytes stay identical (to_msgpack is to_vec_named of the same struct); there are no public signature changes; ClientError::Protocol and the other Protocol(String) shape-mismatch sites are unchanged; shamir-connect Error::Encoding(String) and RequestEnvelopeRef::to_msgpack are unchanged. Regression test rv_client_35a189a9_envelope_encode_error_keeps_typed_source: feed the helper a Serialize impl that returns S::Error::custom, then assert matches!(err, ClientError::Encode(_)) and that std::error::Error::source(&err) is Some. Out of scope: retyping Protocol(String), and changing the shamir-connect Error enum.
- Evidence at `07bc804c`: crates/shamir-client/src/error.rs:11-44; crates/shamir-connect/src/common/messages/envelope.rs:107-109; crates/shamir-client/src/client/api.rs:509-511
- Recipe: Preserve envelope encoding errors in a source-bearing variant and test the source chain.
- Verifier note: Envelope encoding loses its typed source in the shared helper before client stringification; repair both layers and test the source chain.

### L3-RV-CLIENT-8997B366 - `ResumeOptions::ticket` and its wire copy are not `Zeroizing`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Documented limitation, consistent with the P2-09 decision to preserve ResumeOptions.ticket: Vec<u8> for struct-literal callers. SecureResumeOptions/resume_secure and the wire/scratch/rotated-ticket protections are in place (options.rs:96-139, connect.rs:620-629). The residual exposure is a retained legacy options value or a never-polled legacy future; it is already documented at options.rs:98-102 and connect.rs:620-621, and the raw Vec is caller-created. Deprecating Client::resume would require a new #[allow(deprecated)] on legacy_resume_early_error_wipes_real_input_ticket, which the rust-guard structure gate forbids, and removing it is a breaking SDK change with no in-workspace production caller. Record: deprecate or remove the legacy raw-ticket entry point in the first breaking client release.
- Evidence at `07bc804c`: crates/shamir-client/src/client/options.rs:101-126; crates/shamir-client/src/client/connect.rs:623-629,725-739; crates/shamir-transport-tcp/src/framing.rs:198-203
- Recipe: Retire or migrate the legacy raw-ticket resume entry point under an explicit compatibility decision.
- Verifier note: Secure options and wire/framing buffers are protected; retained legacy options and never-polled legacy futures still own raw ticket vectors.

### L3-RV-CLIENT-B80C0D09 - `std::sync::Mutex` on the demux hot paths (`PendingMap`, `SubscriptionMap`, `EarlyBuffer`) without a sanctioned-category justification

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/decision`; **OWNER DECISION**.
- **Decision: ESCALATE** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Owner must decide the CLAUDE.md sanctioned-mutex policy for SubsInner (crates/shamir-client/src/client/transport.rs:251-260, locked per push frame at :592-595). The pending registry is already scc. Option A (recommended, S): add a 4th sanctioned std::sync::Mutex category to CLAUDE.md, 'per-connection client demux state: single reader task plus rare control-path calls (subscribe/unsubscribe/shutdown), O(1) critical sections, never held across .await, poison-tolerant', naming SubsInner. No code change beyond the existing inline contention comment. Option B (M): redesign onto one scc::HashMap<u64, SubSlot, THasher> (SubSlot = Early(Vec<PushEnvelope>) | Live{tx, terminal}), an AtomicUsize aggregate early-buffer counter and the ClosedState publish-then-drain plus re-check-after-insert protocol, with regression tests for push ordering across subscribe/flush and for close-vs-subscribe. Technical merit favours A: scc also takes bucket locks, so there is no throughput gain, and B risks the P0 close-vs-register invariants. CLAUDE.md forbids citing precedent and executors may not edit the policy. Default if unanswered: no change.
- This card waits for the owner: do not implement it; report it as SKIPPED(owner).
- Evidence at `07bc804c`: crates/shamir-client/src/client/transport.rs:244-261,588-595; CLAUDE.md:607-650
- Recipe: Authorize a specific subscription-lock policy exception or design a replacement preserving atomic routing and lifecycle transitions.
- Verifier note: Pending registry migrated to scc, but push routing still uses an unsanctioned standard mutex; redesign is M or an authorized policy exception is S.

### L3-RV-TRANSPORT-IPC-FA481516 - chmod-after-bind race window (documented, spec-prescribed — report as accepted risk with a structural option)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted risk, consistent with the P2-18 SKIP and already documented in docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:61-66, with the recommendation of an effective-user-owned 0700 socket directory. bind_with already checks parent ownership and non-writability before and after bind and fails start if chmod fails (crates/shamir-transport-ipc/src/unix.rs:47-120). Requiring a 0700 parent would break deployments with traversable parents. A umask-based fix is unsafe because umask is process-global. A connection made in the interval still has to complete full SCRAM (TRANSPORT_UNIX.md:50-54).
- Evidence at `07bc804c`: crates/shamir-transport-ipc/src/unix.rs:69-111; docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:61-66
- Recipe: Retain accepted conditional exposure or require a server-owned 0700 socket parent before bind.
- Verifier note: Bind publishes before chmod and accepts traversable trusted parents; retain conditional acceptance or require a private parent with coverage.

### L3-RV-TRANSPORT-TCP-31C5E8A6 - Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams

- Verdict: `CHANGED`; risk `low`; fix size/kind `M/test`.
- Evidence at `07bc804c`: crates/shamir-transport-tcp/src/framing.rs:52-66,98-107; crates/shamir-transport-tcp/tests/fixtures/measurement.md:23-26
- Recipe: Implement the documented CI plain-TCP syscall and latency measurement before optimizing bounded reads.
- Verifier note: Old read_exact pair is gone; split bounded reads remain, and the CI measurement harness/results are not implemented.

### L3-RV-TRANSPORT-TCP-79910BA5 - Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-client/src/client/api.rs:278,504-513,583; crates/shamir-transport-tcp/src/framing.rs:187,199-201
- Recipe: Serialize into reusable zeroizing preframed storage while preserving write serialization and terminal cancellation.
- Verifier note: Reachable production roundtrip allocates and copies an already serialized envelope into a second frame buffer.

### L3-RV-TRANSPORT-TCP-884DDBF5 - `write_frame_prereserved` reports every contract violation as `TooLarge`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/-`.
- Evidence at `07bc804c`: crates/shamir-transport-tcp/src/framing.rs:278-280,298-315
- Verifier note: First pass missed oversized mismatched prefixes: the cap check returns TooLarge before body-length validation, contrary to the documented malformed-buffer contract.

### L3-RV-TRANSPORT-TCP-A38C5274 - `tls.rs` has zero unit tests; all error branches unexercised (happy-path-only e2e coverage)

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/test`.
- Evidence at `07bc804c`: crates/shamir-transport-tcp/src/tests/tls_tests.rs:15-21,49-65; crates/shamir-transport-tcp/src/tests/verifier_tests.rs:89-106
- Recipe: Add a real uncompleted rustls-connection exporter case asserting its specific error cause.
- Verifier note: Typed PEM/key/mismatch tests exist; incomplete-handshake exporter coverage still needs a real connection seam rather than injected General errors.

### L3-RV-TRANSPORT-TCP-17E11DF1 - Project-specific exporter is incorrectly described as strict RFC 9266 compliance

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-transport-tcp/src/tls.rs:57; docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:154; docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:41
- Recipe: Qualify the remaining TCP-spec RFC reference as project-specific without changing wire bytes.
- Verifier note: Qualify the remaining TCP-spec RFC reference as project-specific without changing exporter bytes.

### L4-RV-CLIENT-1C219006 - Ambient interner sync runs on every `execute` regardless of server version or cache state

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/-`.
- Evidence at `07bc804c`: crates/shamir-client/src/client/api.rs:260-270; crates/shamir-client/src/receipts/client_ops.rs:33-39,85-90
- Verifier note: First pass missed public execute_with_receipts, which still creates interner cache entries without a legacy-peer version gate.

### L4-RV-CLIENT-66E9A5FC - Perf claims are unmeasured: no benches, no allocation-behaviour tests

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `07bc804c`: crates/shamir-client/Cargo.toml:42-55; docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md:32,63
- Recipe: Add allocation-count oracles and fixed-iteration client benchmarks with results recorded from CI.
- Verifier note: Client allocation-count oracles and implemented benchmark measurements remain absent and explicitly deferred.

### L3-RV-TRANSPORT-TCP-93246CAF - Test name promises TLS-bind-on-unspecified coverage the body deliberately doesn't deliver

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/test`.
- Evidence at `07bc804c`: crates/shamir-transport-tcp/src/tests/listener_tests.rs:15-24,138-142
- Recipe: Rename the old loopback test accurately while retaining the new unspecified-bind regression.
- Verifier note: Actual unspecified-bind coverage exists, but the older loopback test still has a misleading unspecified-address name.

### L4-RV-CLIENT-NODE-31D3BBE5 - Version drift: Cargo `0.1.0-alpha.1` vs npm/generated-loader `0.1.0`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted and deferred to release preparation, same as the P2-03 SKIP. Cargo 0.1.0-alpha.1 (publish=false) vs npm 0.1.0 does not affect loading: native/index.js:211-214 compares only npm platform-package versions, and only when NAPI_RS_ENFORCE_VERSION_CHECK is set. Aligning versions is a version bump, which executors may not do. Declaring independent versioning is an owner release-policy statement that has not been recorded. No code or doc change now.
- Evidence at `07bc804c`: crates/shamir-client-node/Cargo.toml:3; crates/shamir-client-node/package.json:3; crates/shamir-client-node/native/index.js:211-214
- Recipe: Decide and document version alignment or intentional independent versioning during release preparation.
- Verifier note: Version divergence remains unexplained; a release-policy note suffices and Cargo metadata does not drive loader rejection.

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

`cargo fmt -p shamir-client -p shamir-client-node -p shamir-transport-tcp -p shamir-transport-ipc -- --check`; `cargo clippy -p shamir-client -p shamir-client-node -p shamir-transport-tcp -p shamir-transport-ipc --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
