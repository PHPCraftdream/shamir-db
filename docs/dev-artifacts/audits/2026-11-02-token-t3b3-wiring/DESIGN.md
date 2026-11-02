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

## Revision 1 progress

- Reviewer correction: the HTTP tests use the public query-builder `Query` re-export, proxy boundary assertions and formatting are restored, TLS config helper formatting is corrected, and rollback log strings follow rustfmt. `tls13_config` is public for launcher preflight, overriding the earlier no-crate-public-API decision for this revision.

- Item 1: DONE (source). Added `http_rest_shutdown_bounds_incomplete_request_body_and_reports_error` in `crates/shamir-server/tests/e2e/http_rest/lifecycle.rs`: uses a valid ReadOnly harness token, a Content-Length within the limit, reads/asserts complete interim 100 response before withholding the body, checks the exact drain error, applies a 30-second shutdown bound, and probes EOF/reset. Source only; CI must validate runtime behavior. Existing keep-alive/restart test's shutdown assertion, temp lifetime, and closing brace were restored.

- Item 2: DONE (source). `common.rs` now loads owner roles from the persisted directory state for token records (the bootstrap admin's real roles remain empty), `policy.rs` uses the same roles, `rails.rs` creates `items` over the native client and requires a successful HTTP insert batch, and proxy tests use oversized valid IP entries plus an under-limit valid control. CI compilation and behavioral validation remain pending; no tests or checks run locally.

- Item 3: DONE (source). HTTP REST e2e shutdown assertions now check `report.errors().is_empty()` except the keep-alive shutdown/restart test, which retains `is_clean()` and uses a 30-second shutdown timeout; incomplete-body error assertion and other 15-second socket/request bounds remain unchanged. CI validation pending.

- Item 2 follow-up: DONE (source). `common.rs` documents `launch_with_native_seed_settings`; `rails.rs` now verifies the inserted row via an HTTP `Batch.query` using the same token. CI validation pending.

## Test inventory

- `http_rest_scope_denial_and_read_only_write_return_forbidden` — `crates/shamir-server/tests/e2e/http_rest/policy.rs`
- `http_rest_remote_plaintext_opt_in_allows_executor_after_write_rail` — `crates/shamir-server/tests/e2e/http_rest/rails.rs`
- `http_rest_unknown_bad_mac_revoked_expired_are_uniform_unauthorized` — `crates/shamir-server/tests/e2e/http_rest/authentication.rs`
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

### Final response (historical)

PARTIAL — implemented HTTP launcher wiring, TLS/plaintext rails, trusted-proxy handling, maintenance shutdown ordering, and eight real-launcher REST test sources. Changes are in server_launcher/http/, launch.rs, listeners.rs, server_handle.rs, server_tests.rs, tests/e2e/http_rest/, tests/e2e/main.rs, and this audit note. Final cargo fmt --check, tracked layout, and git diff --check passed; no compilation, clippy, rust-guard, or behavioral tests were run. Remaining: the launcher directory has eight entries (limit seven), requiring an authorized move; CI must compile/lint/run the tests. No Cargo.toml/Cargo.lock edits, file moves/deletions, commits, or pushes.

## Revision 1 item 4 progress

- Item 4: DONE (source; CI pending). Corrected mapped SocketAddr syntax, added mapped trusted-intermediate and mapped-with-port forwarding coverage, and kept IPv6 CIDR prefixes as IPv6. CIDR tests cover mapped /120 boundary, mapped IPv4 conversion, and genuine IPv6 boundary. `listeners.rs` async loop body is manually indented and HTTP's post-entry match arm is unreachable; HTTP TLS config is now preflighted with the documented public `tls13_config` helper before spawn while serve retains the same TLS 1.3, HTTP/1.1 ALPN, and disabled 0-RTT policy; proxy peer and forwarded IPs are canonicalized with mapped-peer IPv4 CIDR matching, preserving IPv6 CIDR prefix interpretation. `tls12_only_configuration_is_rejected_before_accept_loop` and focused mapped address/CIDR boundary proxy tests are source additions; not run locally.

## Revision 1 report

**Overall: PARTIAL (source complete where noted; runtime validation pending CI).** Revision 1 implements items 1–4 in source, with item 4f deferred under the permitted fallback. Parent checks: `cargo fmt -p shamir-server -- --check`, `python -I scripts/ci/check_layout.py`, and `git diff --check` all exited 0. Tests, compilation, clippy, and rust-guard were not run for this revision; children ran no commands. These checks establish no runtime behavior.

| Item | Status | Report |
|---|---|---|
| 1 | DONE (source); PARTIAL (validation) | `server_handle.rs:192` applies a shared 10-second deadline and 2-second abort bound, reports the exact error, and quiesces the native path; listener rollback at `listeners.rs:378` and `launch.rs:716` preserves same-bound logging. Added `http_rest_shutdown_bounds_incomplete_request_body_and_reports_error` (`lifecycle.rs:229`): valid token, `100 Continue` handshake, withheld in-limit body, 30-second shutdown bound, and EOF check without short body/request timeouts. CI must validate execution. Aborting a transport JoinSet requests child abort on drop but does not explicitly join every child; EOF is source-covered, not proof of runtime quiescence. Existing audit spawn may still outlive the unchanged 2-second bound. |
| 2 | DONE (source); PARTIAL (validation) | `common.rs:123,137` snapshots actual roles and `policy.rs:16` uses them; bootstrap admin roles remain empty, not fabricated as superuser. `rails.rs:45,84,107` uses native item creation, requires successful batch insertion, and reads back a marker. Proxy tests (`proxy_tests.rs:82`) include valid oversized entries and an under-limit control. |
| 3 | DONE (source); PARTIAL (validation) | REST shutdown errors are asserted empty except the audit keep-alive case, which checks `is_clean()` (`audit.rs:35,104,124`) with a 30-second shutdown bound (`audit.rs:101`). Existing unit tests remain untouched. CI validation pending. |
| 4 | DONE (source); PARTIAL (validation) | Listener indentation and unreachable HTTP arm corrected (`listeners.rs:185`, `crates/shamir-server/src/engine/server/server_launcher/listeners.rs:259`). Launcher preflights TLS via the documented public `server::accept::tls13_config` helper before spawn (`http/launcher.rs:59`); serving retains TLS 1.3, HTTP/1.1 ALPN, and disabled 0-RTT. Proxy canonicalizes peer/forwarded addresses and handles mapped IPv4 CIDRs while preserving IPv6 prefix interpretation (`http/proxy.rs:42,126`); coverage includes mapped chains and CIDR (`proxy_tests.rs:48,94`). Deployment guidance updated in `deploy/server.example.ktav:87`. `seed_owner` remains open/drop because no live user directory is exposed by the handle; roles are read during that same open. This is the permitted fallback, not a completed lifecycle change. No API widening to address it. |

New focused source tests include `http_rest_shutdown_bounds_incomplete_request_body_and_reports_error`, `mapped_peers_and_forwarded_addresses_resolve_to_canonical_ipv4`, `direct_untrusted_mapped_peer_falls_back_to_canonical_ipv4`, and `valid_forwarded_header_under_limit_is_parsed`; the existing TLS-12 rejection test now checks helper and serve failure. No test was run.

Manual layout: `server_launcher/`, `http/`, and `tests/e2e/http_rest/` each have seven entries. Earlier eight-entry parent-folder statements are historical and superseded; no move is needed. The public `tls13_config` addition is the narrow documented exception to the earlier no-public-API decision; locked dependency versions were inspected, not runtime-verified. The 16 modified files are existing files only; manifests, lockfile, protected unit tests, and file layout were not changed.

Implemented revision 1 items 1–4 in source: bounded listener shutdown and rollback, stronger REST/proxy oracles, Windows-safe shutdown assertions, TLS preflight, mapped-IP normalization, and updated deployment guidance. Changed server_handle.rs, launcher/listener/proxy files, HTTP REST e2e tests, two transport HTTP files, deploy/server.example.ktav, and this audit note. Server fmt --check, layout, and git diff --check pass. PARTIAL validation: compilation, clippy, rust-guard, and behavioral tests require CI. The optional seed_owner lifecycle change remains deferred under the brief’s permitted fallback. No manifests, lockfiles, protected unit tests, file moves/deletions, commits, or pushes.
