# P2-18 - shamir-server + transports (tcp, ws, ipc): origin policy, loopback, exporter docs, test maintenance

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the 31 findings below in one coherent change set for shamir-server, shamir-transport-tcp, shamir-transport-ws, shamir-transport-ipc (effort units 50: S=1, M=2, L=4). They are adjacent: same crate and overlapping files, so they are solved together and share tests and review. TCP pooling must keep secret zeroization and terminal cancellation behavior.

## Scope

Crates: `shamir-server`, `shamir-transport-tcp`, `shamir-transport-ws`, `shamir-transport-ipc`. Dependents are type-checked with `cargo check` when a public signature changes.

## Decisions

Filled by the orchestrator before launch (owner answers). Empty means: skip the cards marked OWNER DECISION: E8E1D11B, A65118CC, FA481516, F0A54173, 97C240D7, 4D95A5F8, 9C706A16, 460A02D6, 5D3BB859, B1243564.

- (none yet)

## Findings (ordered by risk, then size)

### RV-SERVER-E8E1D11B - Replication client uses trust-on-first-use with no leader-key pinning

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-server/src/sync/replication/transport/prod_factory.rs:114-117; crates/shamir-connect/src/client/handshake.rs:263-294
- Recipe: Define persisted leader-pin enrollment, rotation, mismatch, and reconnect policy and wire it into replication.
- Note: Identity continuity is absent; password-derived and exporter-bound authentication remain.

### RV-SERVER-A65118CC - Duplicated normative loopback predicate with shamir-transport-tcp

- Verdict: `CHANGED`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-server/src/boot/config.rs:939,991-995; crates/shamir-transport-tcp/src/listener.rs:51-55
- Recipe: Choose mapped-IPv4 loopback policy and align or contract-test both validation paths.
- Note: Upgraded from nit: transport accepts mapped loopback while server configuration rejects it.
- Second pass: Mapped IPv4 loopback is accepted by transport but rejected by server configuration; choose and contract-test one policy.

### RV-SERVER-F60C70B4 - Concurrent namespace updates can clobber each other

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-server/src/security/observability/logging/mod.rs:170-173
- Recipe: Use a retrying ArcSwap RCU update and add a synchronized concurrent-override preservation regression.
- Note: Observed load-clone-store allows statically inferred lost updates.
- Second pass: Public load-clone-store permits lost independent overrides; use retrying RCU and synchronize the actual competing update window in the regression.

### RV-TRANSPORT-IPC-FA481516 - chmod-after-bind race window (documented, spec-prescribed — report as accepted risk with a structural option)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-transport-ipc/src/unix.rs:66-82,91-116; docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:57-59
- Recipe: Record accepted conditional exposure and offer an owner-only socket parent as structural mitigation.
- Note: Parent hardening narrows risk but publication still precedes chmod.

### RV-TRANSPORT-TCP-17E11DF1 - Project-specific exporter is incorrectly described as strict RFC 9266 compliance

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/tls.rs:58; docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:154
- Recipe: Describe the project-specific exporter separately from RFC 9266 without changing wire bytes.
- Note: Standards-description mismatch, not demonstrated cryptographic weakness.

### RV-TRANSPORT-TCP-31C5E8A6 - Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams

- Verdict: `CHANGED`; risk `low`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/framing.rs:52,98; 6a128476
- Recipe: Measure plain-TCP syscall counts and latency in CI before changing bounded framing reads.
- Note: read_exact pair replaced; split staging remains, performance consequence unmeasured.
- Second pass: Staged bounded reads remain after read_exact removal; measure real syscall counts and latency in CI before optimizing.

### RV-TRANSPORT-TCP-79910BA5 - Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/api.rs:571; crates/shamir-transport-tcp/src/framing.rs:198
- Recipe: Reuse per-connection zeroizing framing storage while preserving cancellation safety.
- Note: Production request writes still allocate and copy.

### RV-TRANSPORT-WS-67E355C7 - `accept_browser_ws` clones the origin policy on every connection

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-transport-ws/src/browser.rs:22-24; crates/shamir-transport-ws/src/server.rs:223; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:443
- Recipe: Borrow the transport callback policy and share immutable policy storage across connection tasks.
- Note: Deep clones remain; runtime cost is unmeasured.

### RV-TRANSPORT-WS-F0A54173 - Wildcard origin matcher is raw string logic — accepts literal `*` and userinfo forms; unvalidated patterns fail silently

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-transport-ws/src/browser.rs:37-39,59-70
- Recipe: Validate origin patterns and serialized origins while preserving explicit one-level wildcard semantics.
- Note: Malformed wildcard and userinfo forms still satisfy raw suffix matching.

### RV-SERVER-8999F6B7 - Positional handshake DTOs independently mirrored with shamir-client

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-server/src/sync/connection/session/wire.rs:38-99; crates/shamir-client/src/client/transport.rs:721-745
- Recipe: Add shared byte-exact handshake compatibility fixtures for base and hinted positional frames.
- Note: Independent definitions remain aligned; no present wire incompatibility established.

### RV-SERVER-97C240D7 - `bootstrap_password` accepted as a CLI argument

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-server/src/main.rs:48-56,414
- Recipe: Choose protected-file/stdin credential input and deprecate or explicitly document plaintext argv input.
- Note: Optional local command-line/history exposure remains launch-method and permission dependent.

### RV-TRANSPORT-IPC-400D803E - Bind-collision error text is OS-asymmetric and unmapped

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-transport-ipc/src/unix.rs:150-169; crates/shamir-transport-ipc/src/windows.rs:91,160-168; d4981f5e (#360)
- Recipe: Add endpoint and first-instance diagnostic context while preserving OS error identity and ambiguity.
- Note: Unix collision diagnostics improved; Windows still propagates raw creation errors.
- Second pass: Unix collision diagnostics improved; Windows first-instance creation still lacks endpoint context.

### RV-TRANSPORT-TCP-16C543F3 - Transport wire structs and fixtures duplicated across the two e2e test files

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/tests/handshake_e2e.rs:46,89; crates/shamir-transport-tcp/tests/echo_e2e.rs:58,122
- Recipe: Share handwritten-peer wire fixtures without replacing independent protocol oracles.
- Note: Both suites still duplicate wire structs and KDF/user fixtures.

### RV-TRANSPORT-TCP-4D95A5F8 - Write-side frame cap is hardcoded to `MAX_FRAME_SIZE_DEFAULT`; reader/writer API asymmetry

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/framing.rs:80,187,230,295
- Recipe: Add capped writer companions while retaining existing default-cap wrappers.
- Note: Reader/writer cap API asymmetry remains.

### RV-TRANSPORT-TCP-5CE7BA5B - e2e tests run Argon2id inline on the test runtime thread while a peer task is spawned (fragile, no live failure)

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/tests/handshake_e2e.rs:117,283; crates/shamir-transport-tcp/tests/echo_e2e.rs:377; crates/shamir-connect/src/client/handshake.rs:236
- Recipe: Offload synchronous challenge derivation from the test runtime worker.
- Note: Inline Argon2 worker occupancy remains; no live deadlock established.

### RV-TRANSPORT-TCP-B48A0EF2 - Sync CPU-bound bootstrap helpers lack a spawn_blocking / bootstrap-only contract (pillar 2, letter)

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/tls.rs:62,80; crates/shamir-server/src/security/tls.rs:91,115
- Recipe: Document bootstrap scheduling and require spawn_blocking for active async-runtime callers.
- Note: Synchronous helpers still lack scheduling guidance; no per-request starvation shown.
- Second pass: Synchronous bootstrap helpers lack scheduling guidance; inspected production use precedes accept-loop spawning.

### RV-TRANSPORT-WS-5AF6EE84 - Error-semantics / doc-accuracy warts

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-ws/src/framing.rs:53,239-242; crates/shamir-transport-ws/src/server.rs:66-79
- Recipe: Document short-prefix actual-length semantics and caller ownership of pre-auth limits.
- Note: Both documented inaccuracies remain; overlaps E6697C98.
- Second pass: Short-prefix actual length and caller-selected pre-auth policy remain inaccurately documented; preserve existing error semantics.

### RV-SERVER-4512C85D - Vacuous dead-code discard of `ConnectError::AuthFailed`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-server/src/sync/connection/request_loop.rs:514
- Recipe: Delete the discarded AuthFailed value and its unused import.
- Note: Discard remains vacuous and does not demonstrate missing error handling.

### RV-TRANSPORT-IPC-9B233842 - Stale doc: "reject_remote_clients … is left at its default" while the code explicitly pins it

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-ipc/src/windows.rs:12-14,164
- Recipe: State that remote-client rejection is explicitly pinned to true.
- Note: The security setting is present; only its narration is stale.
- Second pass: Remote-client rejection is explicitly enabled; only default-setting narration is stale.

### RV-TRANSPORT-TCP-05C670C2 - Function-local `use` statements violate the imports-at-top rule (5 sites)

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/tests/echo_e2e.rs:209
- Recipe: Hoist the remaining echo-peer latency import to the file header.
- Note: Historical framing/TLS local imports removed; one echo import remains.
- Second pass: Echo-peer latency helpers remain body-local imports.

### RV-TRANSPORT-TCP-6062E565 - Exporter doc cluster misdescribes the adapter abstraction

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/tls.rs:124,128,135,148
- Recipe: Document the actual local exporter adapter and its supported stream implementations.
- Note: Failure handling changed, but adapter documentation remains inaccurate.
- Second pass: Documentation names the wrong trait and overstates production implementations; exporter error propagation is a separate improvement.

### RV-TRANSPORT-TCP-9C706A16 - `tokio` dependency pulls `features = ["full"]` in a library crate

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/dep`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/Cargo.toml:14
- Recipe: Narrow Tokio library features and declare test-only runtime capabilities separately.
- Note: Tokio full remains enabled; savings are unmeasured.

### RV-TRANSPORT-TCP-A13A3534 - Docs advertise Unix-domain-socket binds for `Plain`; the API is `SocketAddr`/TCP-only

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/listener.rs:30,76
- Recipe: Distinguish broader UDS policy from these TCP-only bind and validation APIs.
- Note: Plain documentation still advertises an address family the API cannot express.

### RV-TRANSPORT-TCP-CD312411 - Mixed spellings of the same upstream crate in one import block

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/tls.rs:16,18
- Recipe: Use one consistent pki-types import spelling through the rustls re-export.
- Note: Stylistic inconsistency, not demonstrated type incompatibility.

### RV-TRANSPORT-TCP-EC4F02F5 - Spec wire constant duplicated from a direct dependency

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/tls.rs:58; crates/shamir-connect/src/common/primitives/domain_tags.rs:35
- Recipe: Alias the dependency exporter-label constant while preserving the public TCP name.
- Note: Duplicate byte strings currently agree.

### RV-TRANSPORT-TCP-FA40B59D - Speculative dead public consts with cargo-cult `#[allow(dead_code)]`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-transport-tcp/src/listener.rs:98,100
- Recipe: Remove redundant dead_code allowances while retaining the public constants.
- Note: No public API deletion is needed to resolve the allowance debt.

### RV-TRANSPORT-WS-3D98F208 - `BrowserOriginPolicy::empty()` doc references a nonexistent `accept_no_origin` mode

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-ws/src/browser.rs:28-29,99
- Recipe: Remove the nonexistent accept_no_origin exception and document unconditional empty-policy rejection.
- Note: Stale prose only; missing origins are rejected.

### RV-TRANSPORT-WS-460A02D6 - Unused `[dev-dependencies]`: `hex`, `serde`, `serde_bytes`, `rmp-serde`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/dep`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-transport-ws/Cargo.toml:31-35; crate-wide Rust reference search
- Recipe: Remove the unused hex, serde, serde_bytes and rmp-serde dev-dependencies.
- Note: All four declarations remain without Rust-source consumers.

### RV-TRANSPORT-WS-5D3BB859 - Attacker-controlled `Origin` echoed into the HTTP 403 response body

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-transport-ws/src/browser.rs:86,103; crates/shamir-transport-ws/src/server.rs:259-264
- Recipe: Return a fixed browser origin-rejection body while preserving typed diagnostics internally.
- Note: Raw denied-origin reflection remains without an established rendering exploit.

### RV-TRANSPORT-WS-B1243564 - `Origin` header containing obs-text bytes is reported as `Missing` rather than rejected-as-present

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-transport-ws/src/server.rs:244-257; crates/shamir-transport-ws/src/browser.rs:99
- Recipe: Classify present non-text browser origins as NotAllowed rather than Missing.
- Note: Upgrade fails, but present-invalid still becomes absent and HTTP 400.

### RV-TRANSPORT-WS-E6697C98 - Doc drift: `server_ws_config` claims a "4 KiB pre-auth logical check … enforced in `crate::framing::ws_recv_into`" — no such enforcement exists in this crate

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-transport-ws/src/server.rs:66-79; crates/shamir-transport-ws/src/framing.rs:254-258; crates/shamir-server/src/sync/connection/handshake.rs:778
- Recipe: Separate caller-selected phase limits from accept-time tungstenite buffering ceilings in the documentation.
- Note: The adapter applies a supplied limit, not intrinsic authentication policy.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- Cards marked `OWNER DECISION` are implemented only if the "Decisions" section below contains the answer; otherwise skip them and say so in the final report.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-server -p shamir-transport-tcp -p shamir-transport-ws -p shamir-transport-ipc -- --check`; `cargo clippy -p shamir-server -p shamir-transport-tcp -p shamir-transport-ws -p shamir-transport-ipc --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
