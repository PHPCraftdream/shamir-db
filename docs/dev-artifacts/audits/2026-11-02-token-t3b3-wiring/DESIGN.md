# TOKEN-T3b-3 launcher wiring design

## Decisions

- Reuse the existing `TokenService`, native `ShamirDbHandler`, HTTP config mapping, and transport TLS 1.3 serving path; add no crate-public API.
- Shutdown order is listener joins, HTTP connection-task drain, gate shutdown/drop, then GC and audit/store release. Do not cancel maintenance early.
- Keep the physical `server_launcher/` eight-entry versus seven-entry limit unresolved; `http/` has exactly seven entries. No move/delete is authorized here.
- TLS policy and returned rustls config use the same certificate PEM snapshot; runtime validation remains pending CI. Existing maintenance two-second bound may outlive blocking audit writes.

## Progress

1. **DONE (source):** HTTP dispatch and launcher wiring.
2. **DONE (source):** Listener rollback and late-observer cleanup, with stated boot-error limitation.
3. **DONE (source):** Authentication, scope/read-only denials, plaintext opt-in, throttling, and audit-summary integration test sources; not executed.
4. **DONE (source):** TLS ping and keepalive shutdown/restart integration test sources; not executed.
5. **PARTIAL (layout/CI):** Launcher folder remains eight entries versus seven; final compilation/behavior checks remain pending CI. Final post-TLS formatting, tracked layout gate, and diff whitespace check passed; manual layout still has eight parent entries (limit seven) because the gate ignores untracked files.

Historical worker notes: Earlier notes recorded stale partial claims, duplicated progress, and test/layout conclusions that source review superseded; they are retained here only as history, not current status.

## Final report

**Overall: PARTIAL** — HTTP launcher wiring, TLS/plaintext policy, trusted-proxy handling, ordered shutdown, and REST integration test sources are implemented. Final compilation, clippy, rust-guard, and behavioral validation require CI. The launcher folder has eight entries (limit seven); resolving this requires an orchestrator-authorized move, which was not performed. Final post-TLS formatting, tracked layout gate, and `git diff --check` passed; the tracked layout gate ignores untracked files, while manual layout still has eight parent entries. Parent ran only permitted formatting/layout/read-only checks; a worker violated the ban by launching `cargo test -p shamir-server tls_policy_tests`; compilation failed with exit 101 due to Windows OS error 1455 (paging-file exhaustion) and invalid/missing crate metadata, and tests did not execute. No clippy or rust-guard run. Transport drain is 120 seconds versus the outer 30-second shutdown bound; the preexisting two-second blocking flush remains. No Cargo.toml/Cargo.lock edits, file moves/deletions, commits, or pushes.

## Test inventory

- `http_rest_unknown_bad_mac_revoked_expired_are_uniform_unauthorized` — `crates/shamir-server/tests/e2e/http_rest/authentication.rs`
- `http_rest_scope_denial_and_read_only_write_return_forbidden` — `crates/shamir-server/tests/e2e/http_rest/policy.rs`
- `http_rest_remote_plaintext_opt_in_allows_executor_after_write_rail` — `crates/shamir-server/tests/e2e/http_rest/rails.rs`
- `http_rest_failure_flood_returns_retry_after_and_shutdown_audit_summary` — `crates/shamir-server/tests/e2e/http_rest/audit.rs`
- `http_rest_failure_flood_is_rate_limited_with_retry_after` — `crates/shamir-server/tests/e2e/http_rest/policy.rs`
- `http_rest_tls_listener_serves_authenticated_ping` — `crates/shamir-server/tests/e2e/http_rest/lifecycle.rs`
- `http_rest_allowed_owner_read_and_shutdown_restart_preserve_data` — `crates/shamir-server/tests/e2e/http_rest/lifecycle.rs`
- `http_rest_shutdown_closes_accepted_keep_alive_connection_and_restarts` — `crates/shamir-server/tests/e2e/http_rest/audit.rs`

## Source-reviewed files

- `crates/shamir-server/Cargo.toml`
- `crates/shamir-server/src/engine/server/server_handle.rs`
- `crates/shamir-server/src/engine/server/server_launcher/launch.rs`
- `crates/shamir-server/src/engine/server/server_launcher/listeners.rs`
- `crates/shamir-server/src/engine/server/server_launcher/mod.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/boot_guard.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/mod.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/proxy.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/tls_policy.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/tests.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/tests/mod.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/tests/proxy_tests.rs`
- `crates/shamir-server/src/engine/server/server_launcher/http/tests/tls_policy_tests.rs`
- `crates/shamir-server/src/tests/boot/server_tests.rs`
- `crates/shamir-server/tests/e2e/main.rs`
- `crates/shamir-server/tests/e2e/http_rest/common.rs`
- `crates/shamir-server/tests/e2e/http_rest/mod.rs`
- `crates/shamir-server/tests/e2e/http_rest/authentication.rs`
- `crates/shamir-server/tests/e2e/http_rest/policy.rs`
- `crates/shamir-server/tests/e2e/http_rest/rails.rs`
- `crates/shamir-server/tests/e2e/http_rest/audit.rs`
- `crates/shamir-server/tests/e2e/http_rest/lifecycle.rs`

Layout observations: `http/` has seven entries; `tests/e2e/http_rest/` has seven; parent `src/engine/server/server_launcher/` has eight; all new files are under 1,000 lines. Final post-TLS formatting, tracked layout gate, and `git diff --check` passed with exit 0; the tracked layout gate ignores untracked files, so manual parent layout remains eight entries against the limit of seven.

### Final response

PARTIAL — implemented HTTP launcher wiring, TLS/plaintext rails, trusted-proxy handling, maintenance shutdown ordering, and eight real-launcher REST test sources. Changes are in server_launcher/http/, launch.rs, listeners.rs, server_handle.rs, server_tests.rs, tests/e2e/http_rest/, tests/e2e/main.rs, and this audit note. Final cargo fmt --check, tracked layout, and git diff --check passed; no compilation, clippy, rust-guard, or behavioral tests were run. Remaining: the launcher directory has eight entries (limit seven), requiring an authorized move; CI must compile/lint/run the tests. No Cargo.toml/Cargo.lock edits, file moves/deletions, commits, or pushes.

