# P1 stage G6 — p1-transport-tcp

## Goal

TCP/TLS: NoCaVerify, pre-auth allocation, framing errors, tests. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G6): `NoCaVerify` is not reachable in production configuration (or is explicitly opt-in with a warning); frame buffers grow with received bytes, not the declared length; the pre-auth limit is the handshake limit; framing errors are precise and the post-error state documented; TLS error paths and the listed edges are tested.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TRANSPORT-TCP-1257DB79 — `NoCaVerify` disables all server authentication — chain, CertificateVerify proof-of-possession, and hostname — guarded only by a doc comment

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-tcp/src/tls.rs:177; crates/shamir-client/src/client/connect.rs:406; changed by: none
- R0 note (includes the fix site): OBSERVED: TLS13 CertificateVerify still returns an unconditional assertion; production subsequently authenticates the exporter-bound Ed25519 identity, so intentional CA/hostname bypass is not evidence of a deployed identity bypass. The independent TLS proof-of-possession omission remains. Fix site: crates/shamir-transport-tcp/src/tls.rs.

### RV-TRANSPORT-TCP-BBF93E33 — Full-size allocation on declared length before any payload byte arrives (16 MiB amplification per connection)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-tcp/src/framing.rs:94; crates/shamir-transport-tcp/src/framing.rs:142; changed by: none
- R0 note (includes the fix site): OBSERVED: accepted headers still allocate or resize to the entire declared payload before reading any body byte; zero-filling removes UB, not this capacity amplification. Server pre-auth reads remain separately capped at 4 KiB. Fix site: crates/shamir-transport-tcp/src/framing.rs for incremental bounded body accumulation.

### RV-TRANSPORT-TCP-5940216B — Production Rust client applies the data-frame limit before server authentication

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/client/connect.rs:297; crates/shamir-client/src/client/connect.rs:350; changed by: none
- R0 note (includes the fix site): OBSERVED: challenge and auth_ok still use the 16 MiB data cap before process_auth_ok authenticates the server; the new whole-handshake deadline does not enforce the 4 KiB ceiling. Fix site: crates/shamir-client/src/client/connect.rs, applying the pre-authentication cap to these reads.

### RV-TRANSPORT-TCP-A38C5274 — `tls.rs` has zero unit tests; all error branches unexercised (happy-path-only e2e coverage)

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-transport-tcp/src/tests/tls_tests.rs:36; crates/shamir-transport-tcp/src/tests/tls_tests.rs:46; changed by: 94e27d7e (#276)
- R0 note (includes the fix site): OBSERVED: registered unit tests now assert exact Pem and NoKeyInPem variants, refuting the zero-unit-tests and entirely-happy-path premise; certificate/key mismatch and exporter-error causes still lack discriminating assertions. Fix site: crates/shamir-transport-tcp/src/tests/tls_tests.rs for those remaining branches.

### RV-TRANSPORT-TCP-93246CAF — Test name promises TLS-bind-on-unspecified coverage the body deliberately doesn't deliver

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-tcp/src/tests/listener_tests.rs:101; crates/shamir-transport-tcp/src/listener.rs:75; changed by: none
- R0 note (includes the fix site): OBSERVED: the test named for an unspecified TLS bind still supplies 127.0.0.1; separate allows assertions do not exercise bind_validated with an unspecified TLS address. Fix site: crates/shamir-transport-tcp/src/tests/listener_tests.rs.

### RV-TRANSPORT-TCP-AC0BEB92 — Write-path boundary branches, EOF-vs-close distinction, and IPv4-mapped-IPv6 edge untested

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-transport-tcp/tests/framing.rs:53; crates/shamir-transport-tcp/src/tests/listener_tests.rs:14; changed by: 3084dfe8 (#286)
- R0 note (includes the fix site): OBSERVED: new assertions distinguish boundary PeerEof, zero-length PeerClose and truncated-body/header UnexpectedEof; writer-cap boundary oracles and IPv4-mapped-IPv6 policy cases remain absent. Fix sites: crates/shamir-transport-tcp/tests/framing.rs and crates/shamir-transport-tcp/src/tests/listener_tests.rs for the remaining cases.

### RV-TRANSPORT-TCP-D1882068 — `extract_tls_exporter` collapses all errors (incl. incomplete handshake) into `None`, inviting constant/absent channel binding

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-tcp/src/tls.rs:102; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:119; changed by: none
- R0 note (includes the fix site): OBSERVED: the generic exporter helper still erases every exporter error into None, and TCP/native-WS server callers substitute zeros after successful TLS handshakes; no ordinary completed-handshake failure is established. The scoped API error-erasure obligation remains, not a demonstrated authentication bypass. Fix sites: crates/shamir-transport-tcp/src/tls.rs and the server accept-loop callers.

### RV-TRANSPORT-TCP-884DDBF5 — `write_frame_prereserved` reports every contract violation as `TooLarge`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-tcp/src/framing.rs:257; crates/shamir-transport-tcp/src/framing.rs:269; changed by: none
- R0 note (includes the fix site): OBSERVED: short buffers and nonzero prefix/body mismatches still return TooLarge even below the maximum; zero-prefix inputs now have EmptyPayload, but neither original diagnostic witness is repaired. Fix site: crates/shamir-transport-tcp/src/framing.rs for distinct malformed-buffer classification.

### RV-TRANSPORT-TCP-FF2983EC — Post-error stream state is undocumented (`TooLarge` desyncs the stream)

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-tcp/src/framing.rs:65; crates/shamir-transport-tcp/src/framing.rs:83; changed by: none
- R0 note (includes the fix site): OBSERVED: oversize rejection consumes the header without draining its body, while public reader documentation still omits the discard-after-error obligation; server error handling terminates rather than retries. Fix site: crates/shamir-transport-tcp/src/framing.rs for both public reader contracts.

### RV-TRANSPORT-TCP-A3B66C24 — Cancel-safety contract lives in the consumer, not the crate

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-tcp/src/framing.rs:48; crates/shamir-server/src/sync/connection/request_loop.rs:231; changed by: none
- R0 note (includes the fix site): OBSERVED: the replacement header loop retains partial progress only inside the dropped future, and neither public reader documents terminal cancellation; the consumer alone documents discarding the connection. Fix site: crates/shamir-transport-tcp/src/framing.rs, documenting cancellation obligations for both readers.

### RV-TRANSPORT-TCP-11F795D1 — Client TLS12 rejection test cannot distinguish a version-selection regression

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-tcp/tests/tls13_only.rs:151; crates/shamir-transport-tcp/src/tls.rs:172; changed by: none
- R0 note (includes the fix site): OBSERVED: the client test still accepts any handshake error and ignores the server outcome, while the TLS12 signature callback independently rejects TLS12. INFERRED: enabling TLS12 negotiation can remain masked by that later rejection. Fix site: crates/shamir-transport-tcp/tests/tls13_only.rs with version-stage assertions and compatible controls.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU1 — `NoCaVerify` in production (RV-TRANSPORT-TCP-1257DB79)

Decided by 2x rush reviewer + @oh (unanimous, 3/3): **option A.** Keep skipping chain and hostname validation (identity is the pinned Ed25519 key plus the exporter-bound application proof), but make TLS 1.3 CertificateVerify real and make the permissive mode explicit.

Facts the executor must start from: only `verify_server_cert` and `verify_tls13_signature` in `crates/shamir-transport-tcp/src/tls.rs` return blind assertions; `verify_tls12_signature` already returns `Err`; the config is TLS 1.3 only; the supported scheme list is hand-written; the builder is exported from `lib.rs`. Production callers: `crates/shamir-client/src/client/connect.rs` (~lines 260 and 623), both fail closed when the exporter is missing and use `BindingMode::TlsExporter`.

What the application proof protects (document it in the module docs, do not overclaim): a TLS-terminating relay has a different exporter on each leg, so the relayed SCRAM/identity proofs fail at the other end. What it does NOT protect: `auth_init` (username, client nonce) and `client_proof` are sent before the server is authenticated; the CertificateVerify check does not stop a relay that holds the key of its own certificate, and it is NOT a MITM defence — the exporter-bound proof is.

Acceptance:
1. Rename `NoCaVerify` (for example `AppAuthenticatedServerVerifier`); it holds the crypto provider's `signature_verification_algorithms`. `verify_tls13_signature` delegates to `rustls::crypto::verify_tls13_signature`; `supported_verify_schemes` returns the provider's `supported_schemes()`; `verify_tls12_signature` keeps returning `Err`; `verify_server_cert` keeps returning the assertion with a doc comment that chain and hostname are skipped on purpose.
2. Delete `make_client_config_no_ca` with no alias. The replacement takes a call-site opt-in token (for example `make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())`; zero-sized type with a private field) whose doc lists the caller's duties: obtain the exporter fail-closed, bind it into SCRAM, verify the Ed25519 identity. Update the `lib.rs` export; migrate every caller (production connect and resume paths and the test files); a grep for the old name returns 0 hits.
3. Production connect AND resume stay gated on the exporter-bound proofs: no fallback to an absent or zero binding, no usable session or ticket use before verification; pin mismatch rejected; TOFU needs explicit consent and is persisted only after every check passes. A certificate key is never a substitute for the Ed25519 identity pin.
4. Verifier unit tests (new topic file under `crates/shamir-transport-tcp/src/tests/`, registered in the manifest): (a) valid TLS 1.3 signature from an rcgen key is Ok; (b) tampered message Err; (c) key B signing against cert A Err; (d) unsupported scheme Err; (e) scheme list equals the provider's; (f) a self-signed certificate with a mismatched `ServerName` is accepted; (g) TLS 1.2 Err. Negative cases must exercise the verifier, not fail in fixture construction.
5. Transport integration test in `crates/shamir-transport-tcp/tests/`: a server with a custom `ResolvesServerCert` returning `CertifiedKey(cert_A, signing_key_B)` fails the client handshake (it succeeds on current master); a matched pair is the control and still connects.
6. One e2e test (`crates/shamir-server/tests/e2e/transport/`, keep the 7-entry folder limit): a TLS-terminating proxy with its own self-signed certificate relays frames to a real server. `Client::connect` with a pinned identity returns the handshake error; with `accept_new_host` it returns an error; `Client::resume` fails and the proxy never sees the ticket bytes; no pin is persisted; a direct connection succeeds (control). The failure must be an authentication rejection, not a timeout.
7. Application-handshake unit coverage: a proof valid for exporter X fails for exporter Y; a wrong configured pin fails; a missing pin without explicit first-use permission fails; a failed verification never invokes the pin-persistence callback.

Compat: no wire or disk change; servers that sign CertificateVerify with the key matching their certificate (`with_single_cert`) keep working. Rust API of `shamir-transport-tcp` breaks inside the workspace only. The Node binding goes through `connect_host`.

Out of scope for this card (file as separate follow-ups, do not widen): no client-side minimum for KDF parameters (a relay can pick weak parameters and attack the captured proof offline); replication connects with TOFU and no pin on every connection; username disclosure before server authentication.


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
