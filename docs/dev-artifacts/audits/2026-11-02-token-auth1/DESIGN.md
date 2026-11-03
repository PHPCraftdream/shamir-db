
# TOKEN-AUTH1 revision 1 design and progress

## Design scope

- **WP-A:** Connect-side scope, transcript, proof, pair-status, and caps.
- **WP-B:** Server configuration and listener metadata.
- **WP-C:** HTTP transport plumbing only; this does not implement the login protocol end to end.

## Module map

- Connect: `crates/shamir-connect/src/server/auth/http_login/scope_bytes.rs`, `transcript.rs`, `proof.rs`, and their `tests/` directory.
- Server boot/config and listener/token/meta: `crates/shamir-server/src/boot/config/login_listener.rs`, `http_listener.rs`, `tokens.rs`, and `crates/shamir-server/src/engine/server_meta.rs`.
- Transport: `crates/shamir-transport-http/src/server/routes/login/login_routes.rs`, `login_head.rs`, `login_response.rs`, `no_login.rs`, `route_handler.rs`, plus `GuardedIo`.

## Memory and concurrency (future design; NOT IMPLEMENTED)

Pending 1024 x 2048; arrival/credit 4096 x 128 each; indexes 2 x 1024 x 64; five-prefix 1024 x 64; reserve 65536; lockout 8192 x 128. Total 4,718,592; input 12,582,912; output 4,194,304; audit 1,048,576. These are design figures, not a measured or claimed budget. Future bounded tables would use `tokio::Mutex` and no-await critical sections; existing metadata `write_lock` is for serialized durable ID updates, with atomic write/override. These future facilities are not implemented.

## Public API

Observed API surface: `LoginListenerSettings` fields, `login` / `external_tls`; `TokensConfig.login_ttl` / `login_allow_read_write`; `SecurityConfig.login_enumeration_hardening`; `ServerMetaStore.http_login_listener_id`; transport `LoginRoutes`, `LoginHead`, `LoginResponse`, `NoLogin`, and `serve_with_login`; cfg(test) pub(crate) ID setter. Retained `pub(super)` record, `new_token`, and service helpers from the initial slice; no new production visibility widening.

## Decisions, deviations, and deferred work

Async admission (`RPITIT` associated `Permit` and RAII) was chosen over synchronous brief admission to support a future bounded mutex. Legacy connect `tests.rs` was initially accidentally overwritten but is now mounted as an extra boundary test file with explicit `#[path = "tests/mod.rs"]`; no deletion. The accidental empty `config/login_listener_tests.rs` and unmounted transport `login_tests_mount.rs` are preserved for the operator to remove. FINISH also requires future padding 50 plus uniform 0..25ms. Services, wire protocol, admission, pending state, launcher, and end-to-end work are deferred to revision 2 and are not implemented.

## Chronological progress (reconstructed from observed session)

The previous append worker destroyed the earlier history by overwriting the note; this section reconstructs the record from the observed session, not from an untouched original. Work started 22:48:38 CET Nov 2; deadline 01:15 Nov 3; feature cutoff 00:45. WP-A first pass had partial compile/test/fixture/privacy regressions found and corrected repeatedly. Corrections included canonical username, uppercase DB, namespace conversion, scheduler imports, three vectors, topic mount (173/933), and proof schedule. WP-B initial config/meta pass was partial; imports, private fixture, zero-TTL origins, diagnostics, and full `SecurityConfig` literals in five files were fixed. WP-C had multiple partial passes repairing `Context` generic, moved peer/login, syntax, nonexistent field, write override, and body caps. At 00:45 the final read window began. Parent formatting of all three crates and fmt check passed; tracked layout check passed, with eight manual entries and two folders known. No child commands, local compilation, or tests were run; all remain pending CI. No commits, pushes, or Cargo changes.

## Final report

This section supersedes earlier progress and review-window status statements above; those historical statements are preserved as written. At the 00:55 status, the parent-reported deadline remains 01:15.

- **WP-A — DONE, source; CI pending.** Wire-independent scope, transcript, proof/signature helpers, lockout pair-status support, and token-record caps are present. Human review is recommended for protocol-level HMAC handling in `proof.rs`. No dependency or Cargo-file changes were made.
- **WP-B — PARTIAL.** Configuration, validation, and listener metadata source are implemented. Layout cleanup remains required. The accidental empty `crates/shamir-server/src/boot/config/login_listener_tests.rs` must be removed by the operator; this work did not delete it. The mounted legacy `tests.rs` is an intentional boundary and must not be deleted.
- **WP-C — PARTIAL.** Hook, body handling, headers, framing, and timeout source are present. Required coverage remains missing for stalled-body 408, positive 100 Continue, and real TCP; review gaps remain.

The hook's actual API is `LoginRoutes` with associated `Permit: Send + 'static`; `admit_head` returns `impl Future<Output = Result<Permit, LoginResponse>> + Send`, and `handle` returns `impl Future<Output = LoginResponse> + Send`. It also provides `enabled` and `body_limit`. `LoginHead` preserves duplicate raw headers; `LoginResponse` carries exact bytes and optional Retry-After, without Debug body output. Generic `serve_with_login` is added while existing `serve` remains no-login. Async admission is deliberate: it allows a permit future to wait on a bounded mutex.

**Checks and limitations:** Parent-observed `cargo fmt --check` passed for all three crates; the tracked-only Python layout check passed. Manual config/login checks were 8/8 each. No compilation, clippy, or tests were run. The accidental unmounted placeholder `crates/shamir-transport-http/src/server/routes/login/login_tests_mount.rs` also requires operator removal; it was not deleted. The legacy `tests.rs` remains intentionally mounted. Do not infer exhaustive rereading of every touched file.

**Acceptance 1–12:** 1 PARTIAL — transport plumbing only; 2 PARTIAL — strict wire handling deferred; 3 PARTIAL — listener ID/config only, no challenge; 4 PARTIAL — login not implemented; 5 PARTIAL — pair probe only; 6 PARTIAL — caps/flags only; 7 PARTIAL — not implemented; 8 PARTIAL — startup validation only; 9 PARTIAL — probe only; 10 PARTIAL — metrics not implemented; 11 PARTIAL — audit not implemented; 12 DONE, source only — vectors, bounds, and tests are present; CI pending. Other deferred items remain out of scope.

Relevant source references: `shamir-connect` transcript (`transcript.rs:37,51`), scope bytes (`scope_bytes.rs:34`), proof (`proof.rs:6`), lockout pair status (`lockout.rs:610`), and vector test (`transcript_vector_tests.rs:53`); server listener config (`login_listener.rs:8`), HTTP listener validation (`http_listener.rs:80`), token config (`tokens.rs:55`), and metadata (`server_meta.rs:474`); transport login handler (`route_handler.rs:16,119,165,228`), `serve_with_login` (`accept.rs:69`), and guarded I/O (`guarded_io.rs:110,173`). Named tests: `hand_derived_transcript_vectors_and_length_formula`, `proof_uses_scram_schedule_and_rejects_invalid_controls`, `transcript_accepts_minimum_and_maximum_wire_lengths`; `login_defaults_and_nested_unknown_keys`, `http_login_listener_id_is_stable_across_reopen_and_address_scoped`; `login_path_is_routed_before_bearer_authentication`, `declared_oversize_rejected_before_body_poll`, `streamed_oversize_body_is_capped`, `head_rejection_does_not_send_continue_before_admission`, `shared_write_timeout_expires_stalled_canned_rejection`. These tests have not been run.

### Final message

PARTIAL — WP-A source completed; WP-B config/metadata implemented with cleanup outstanding; WP-C transport hook implemented but required coverage/review unfinished. Changed shamir-connect, shamir-server, shamir-transport-http, deploy/server.example.ktav and DESIGN.md. Formatting checks for all three crates and tracked layout check passed; no compilation or tests ran. Two accidental files require operator removal; CI and remaining WP-C work are pending. No commits or pushes.
