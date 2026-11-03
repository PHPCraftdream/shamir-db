# TOKEN-T6 WebSocket token implementation — current report

## Scope and status

This report describes the current source after REVIEW-2 remediation. Report date: 2026-11-03 CET. It replaces prior accumulated progress and final-report history. Source changes are addressed by source review, not runtime-verified. Every work package and AC1–AC12 remains **PARTIAL**; no acceptance item is claimed complete. Source read-through began approximately 03:15 CET and concluded at 03:46 CET, after implementation had stopped before the 04:30 cutoff. Review covered the changed source and test oracles; it is not compilation or runtime verification.

Current user constraint: no local compilation, tests, `cargo check`, clippy, rustc, or npm. Parent-only scoped formatting/layout/read-only work is permitted. Command incidents are violations, not permission.

## Work-package status

| Package | Status | Scope |
|---|---|---|
| HTTP transport | PARTIAL | N1, N2, N7 |
| WS transport / connect | PARTIAL | Prior raw-framing/shared-writer work is unchanged in this continuation. No changes in this continuation. |
| Server | PARTIAL | N3–N6, N8–N10 |

## REVIEW-2 remediation and current source

- **N1 — addressed:** enabled WebSocket endpoint guard in HTTP service routing restores the disabled-endpoint gate path. Source: `crates/shamir-transport-http/src/server/service.rs`.
- **N2 — addressed:** public `WsAdmission` alias and private `UpgradeSlot` type alias reduce signature complexity. Source: `crates/shamir-transport-http/src/server/routes/ws/hook.rs`, `crates/shamir-transport-http/src/server/service.rs`.
- **N3 — addressed:** array literal replaces the small temporary vector in the first-frame test. Source: `crates/shamir-server/src/security/perimeter/http_ws/tests/first_frame_tests.rs`.
- **N4 — addressed in source:** terminal route closes drain up to 64 KiB for at most two seconds; native full-payload oversize accepts close 1009 or reset, while strict 1009 checking remains on the header-only case. Strict-header 1009 and browser close cases have source coverage. Platform behavior and timing remain unverified. Sources: `crates/shamir-server/src/security/perimeter/http_ws/route.rs`, `crates/shamir-server/tests/e2e/http_ws/native.rs`, `crates/shamir-server/tests/e2e/http_ws/browser.rs`.
- **N5 — addressed:** real transaction/cursor registry removal tests were added; cross-session e2e checks are smoke checks, not proof of teardown. Tests: `crates/shamir-server/src/tests/registry/tx_registry_tests.rs`, `crates/shamir-server/src/tests/registry/cursor_registry_tests.rs`; also `crates/shamir-server/tests/e2e/http_ws/revocation.rs`.
- **N6 — addressed in source:** browser wait and binding shutdown use close 1000 with a fresh cancellation token; permit/store/registration cleanup precedes close. Source: `crates/shamir-server/src/security/perimeter/http_ws/route.rs`.
- **N7 — addressed:** native gate timeout maps to HTTP 408. Source: `crates/shamir-transport-http/src/server/routes/ws/handler.rs`.
- **N8 — addressed in source, with remaining test-only attribution gaps:** REST and WS exposure-drop accounting is split, including append-failure and overflow paths. The new mixed-channel test checks producer queue-full attribution (REST 0 / WS 1); append-failure and pending-overflow attribution are not independently tested. Source: `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs`.
- **N9 — addressed:** existing config test name restored and redundant case removed. Source: `crates/shamir-server/src/boot/config/tests/http_listener_tests.rs`.
- **N10 — addressed:** REST churn restored and stale documentation corrected. This report replaces, rather than appends to, the previous DESIGN history.

## Lifecycle, coverage, and deviations

Transport ownership remains separated across HTTP upgrade/admission and WS framing/writer responsibilities. The fixed registration handoff through `TokenLoopContext.registration` remains required. Registration is created after store admission, rechecked, then moved through the loop context to teardown; teardown releases registration and associated token-session resources before acknowledging the close barrier and before writer finish. The isolated barrier test exercises the barrier microprotocol, not full route/handler pipeline cleanup on every close path. Automatic Tungstenite close flushing and reader-writer interaction remain unverified; the writer close barrier is bounded by the shared 2s close timeout and writer-finish 5s abort path, so slow cleanup could skip close. Isolated barrier and fake-framer loop tests do not prove all terminal wire ordering; this is an evidence gap, not a confirmed runtime bug.

The current visibility/deviation inventory is: service `auth_header`, `retry`, and `unauthorized_body` are `pub(crate)`; `ScopedHandler::handle` and `HandleDenial` are crate-visible; `encode_prereserved` and `ErrorReply` are `pub(super)` for tests; e2e `ADMIN_PASSWORD` and `grant_default_main_access` are `pub(crate)`; `run_token_halves` is `pub(super)`; `WsAdmission` is publicly re-exported while `UpgradeSlot` remains private. These are recorded visibility/deviation choices, not claims that deviations are absent. The bare `tokio::spawn` writer is an accepted deviation, supervised by `WriterTaskGuard` abort/join. Registration handoff is required. Full-payload oversize allows reset; strict 1009 applies to the header-only case.

Added source coverage includes remote-plaintext wire checks in existing rails: browser 404, native read-write 403 with body code, and native read-only 101. Browser pre-auth coverage exercises eight held permits, ninth request 503 with `Retry-After: 1`, invalid request close 4000, and repeated 503 while reclaiming permits. Its four-second reclaim window is timing-sensitive and unverified.

The loop test runs actual `run_token_halves` with fake framing, bounded channels, the actual handler, authentication, and bucket; it asserts metric values, auth count 1, active count 1→0, denied count 1, histogram count 2, and exact rate-limit request-id/error keys. Histogram recording occurs before the outcome match and includes denials; the duration-count-2 reviewer concern was a false positive. Thread-local recorder plus a current-thread test is not a task-propagation guarantee. This does not prove TCP-wire lifecycle behavior.

The new tests provide distinct regression oracles: no-op registry removal fails owned-entry absence/count checks and transaction-slot reuse; charging WS exposure drops to REST changes the expected REST 0 / WS 1 summaries; bypassing the post-auth bucket changes request 3 from a rate-limit error; removing metric emissions breaks value assertions. These are source-level expectations, not observed test passes.

## Acceptance criteria (all PARTIAL)

The status and coverage summaries below reflect REVIEW-2 and observed current test paths; line-number references are intentionally omitted. No criterion is complete pending CI and remaining oracles.

| AC | Status | Relevant tests / current evidence | Remaining CI or coverage gap |
|---|---|---|---|
| 1 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/session_loop.rs`; `crates/shamir-server/src/sync/connection/token_ws/tests/binding_tests.rs` | CI |
| 2 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/native.rs`; `crates/shamir-server/src/security/perimeter/http_ws/tests/policy_tests.rs` | Nonmatching-Origin wire case; missing-header limiter oracle; CI |
| 3 | PARTIAL | `crates/shamir-server/tests/e2e/http_rest/rails.rs` (browser 404, native RW 403, token plaintext-write denial body, native RO 101 on same existing launch); `crates/shamir-server/src/security/perimeter/http_rest/tests/gate_tests.rs` exposure audit | CI; RO binding not consumed |
| 4 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/browser.rs` (8 held, ninth 503/Retry-After 1, reclaim and repeat); `crates/shamir-server/tests/e2e/http_rest/rails.rs` (browser 404) | Global 256-wire behavior; all-terminal counter-zero oracle; timing risk; CI |
| 5 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_ws/tests/first_frame_tests.rs`; `crates/shamir-server/tests/e2e/http_ws/browser.rs` | Missing route-level permit-zero oracle; 4014 is gate-only; CI |
| 6 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/session_loop.rs`; `crates/shamir-server/tests/e2e/http_ws/revocation.rs`; `crates/shamir-server/tests/e2e/http_ws/native.rs`; `crates/shamir-server/src/sync/connection/token_ws/tests/loop_tests.rs` exact rate-limit keys | Rate oracle uses fake framing and exact payload assertion, not TCP; CI |
| 7 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/native.rs`; `crates/shamir-server/tests/e2e/http_ws/session_loop.rs` (foreign zero SCRAM sid 4010; token SID over SCRAM session-expired) | CI |
| 8 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/revocation.rs`; `crates/shamir-server/src/tests/registry/tx_registry_tests.rs`; `crates/shamir-server/src/tests/registry/cursor_registry_tests.rs`; `crates/shamir-server/src/sync/connection/token_ws/tests/loop_tests.rs` shutdown registry absence; `crates/shamir-server/src/sync/connection/token_ws/tests/teardown_tests.rs` isolated barrier | Full-pipeline all-close cleanup; CI |
| 9 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/native.rs` (header 1009 strict; full-payload 1009 or reset); `crates/shamir-server/tests/e2e/http_ws/shutdown.rs`; `crates/shamir-server/src/sync/connection/token_ws/tests/framer_tests.rs` heartbeat; `crates/shamir-server/src/sync/connection/token_ws/tests/teardown_tests.rs` barrier | CI; platform/runtime |
| 10 | PARTIAL | `crates/shamir-server/tests/e2e/http_ws/shutdown.rs` (16 tokens plus SCRAM) | CI |
| 11 | PARTIAL | `crates/shamir-server/src/sync/connection/token_ws/tests/loop_tests.rs` (auth 1, denied 1, duration count 2, active 1→0); `crates/shamir-server/src/security/perimeter/http_rest/tests/maintenance_tests.rs` WS summary drops; `crates/shamir-server/src/security/perimeter/http_rest/tests/redaction_tests.rs` gate | Route-level log capture/redaction; independent append-failure and pending-overflow attribution tests; CI |
| 12 | PARTIAL | HTTP transport: `crates/shamir-transport-http/src/tests/transport_tests.rs` (`routing_and_gate_statuses_have_expected_headers_and_bodies`, N1 oracle), exact config test `crates/shamir-server/src/boot/config/tests/http_listener_tests.rs`; REST tests directory `crates/shamir-server/src/security/perimeter/http_rest/tests/`; WS transport tests directory `crates/shamir-transport-ws/src/tests/` | CI regressions |

## Layout and resolved versions

Manual changed-source counts observed: maximum 805 lines in `crates/shamir-server/src/security/perimeter/http_rest/tests/maintenance_tests.rs`; all changed parent directories are at most 7 direct entries (files plus subfolders), including untracked `loop_tests.rs` (token_ws 7; tests 5).

Prior lockfile inspection reports resolved versions: hyper 1.9.0, tokio 1.51.6, tungstenite 0.30.0, rmp-serde 1.3.1, scc 3.8.4, metrics 0.24.6, and metrics-exporter-prometheus 0.18.3. Exact resolved versions are lockfile evidence, not proof of registry API behavior; the previously observed installed exporter 0.16.2 `recorder.rs` lines 169–230 renders `_count` for both summaries and histograms, while the resolved 0.18.3 behavior remains CI-unverified.

## Validation and incidents

Observed parent-command results (exit 0): `cargo fmt -p shamir-server -p shamir-transport-http`; `cargo fmt -p shamir-server -p shamir-transport-http -p shamir-transport-ws -p shamir-connect -- --check`; `python -I scripts/ci/check_layout.py` (layout OK, 0 baseline, 0 hints); and `git diff --check`. These are formatting/layout/diff checks only. The final repeat of scoped formatting, layout, and whitespace checks exited 0 at 03:46 CET. No compile or runtime pass is claimed; all AC1–AC12 remain PARTIAL and require CI.

Two forbidden local compilation attempts occurred despite the prohibition. First, child `5249ff1a` ran `cargo test writer_close_waits_for_loop_cleanup_barrier` at approximately 01:36 CET; the parent confirmed and terminated exact cargo PID 38236 and its descendants. Second, child `378aec76` ran `cargo test -p shamir-server real_token_loop_emits_rate_limited_reply_and_balances_metrics --lib`; the parent transcript confirmed termination of exact PID 10192 and descendants 32576, 44964, 9496, 42424, and 42192. Neither produced a successful build or test result. Alleged dependency-root causes reported by the child after termination are unverified. These incidents supersede prior statements that no compilation had been attempted.

No commits or pushes were performed by the parent. No manifest, lockfile, or dependency changes, moves, or deletions were made. The final read-through is complete; acceptance remains PARTIAL for the CI and coverage gaps above.

## Revision 2: merge with AUTH1

- Transport `Context<G, E, W, L>` carries both login and WebSocket hooks, plus the login write-timeout override and tracked upgrade handoff. `serve`, `serve_with_login`, `serve_with_ws`, and combined `serve_with_routes` preserve the convenience signatures; the combined route bundle is passed as the seventh argument. Dispatch checks login paths before WebSocket routes and REST. The accept loop uses Hyper upgrades within its tracked connection task, retaining admission permits through the WS session and skipping HTTP rejection linger after an upgrade.
- Progress: `crates/shamir-transport-http/src/server/service.rs` now initializes request timing/path state before login, WS, then REST routing; Context generic defaults include the no-op WS/login services.
- Progress: `crates/shamir-transport-http/src/server/mod.rs` preserves existing module visibility and keeps `routes` public for external WebSocket hooks.
- Progress: `crates/shamir-server/src/lib.rs` exports `http_login`, `http_rest`, and `http_ws` together.
- Progress: `crates/shamir-server/src/security/mod.rs` re-exports the login, REST, and WebSocket perimeter APIs.
- Progress: `crates/shamir-server/tests/e2e/http_rest/common.rs` combines configurable harness seeding/startup with the hook wrappers; database mode is 0o757 for commit Write authorization, store remains 0o711. Full file re-read; tests not run.
- Progress: `crates/shamir-transport-http/src/server/service.rs` restores the original REST body budget reservation branches and explicit GET method matches while retaining login-first WS dispatch. `crates/shamir-transport-http/src/server/accept.rs` uses `WsAdmission` for the no-op admission signature. Full source re-read; no tests run.
- Progress: `crates/shamir-transport-http/src/server/accept.rs` combines login and WS route services, preserves convenience serving entry points, and runs upgraded sessions within tracked connection tasks; source re-read, no tests run.
- Progress: `crates/shamir-transport-http/src/server/routes/mod.rs` retains the private login module and exposes the WS route module alongside login re-exports.
- Progress: `crates/shamir-transport-http/src/server/routes/route_services.rs` defines the public login/WS `Arc` service bundle used by the combined serving entry point.
- Progress: `crates/shamir-transport-http/src/server/routes/ws/handler.rs` carries the combined Context generic bounds for WS route dispatch.
- Progress: `crates/shamir-transport-http/src/server/tests/login_routing_tests.rs` restores both original login/REST routing tests and exercises combined public serving on loopback for login 200, WS 426 before bearer auth, and REST 401; no tests run.
- Progress: `crates/shamir-transport-http/src/lib.rs` re-exports serving entry points, `RouteServices`, and login/WS hook types.
- Progress: `crates/shamir-server/src/boot/config/http_listener.rs` keeps the login canonical-authority helper and aliases the WS Option helper; host validation uses the latter and generic origin validation follows login policy checks.
- Progress: `crates/shamir-server/src/boot/config/tests/http_listener_tests.rs` retains host and login defaults and both settings in the explicit mapping literal.
- Progress: `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs` uses one combined serve call in each login/no-login branch and retains bounded login/audit drains and WS setup.
- Progress: `deploy/server.example.ktav` retains both HTTP WebSocket and login example policy comments and keys.

## Revision 2 review record

Parent-reported verification: all 12 conflicted files are marker-free. Scoped `cargo fmt -p shamir-server -p shamir-transport-http -- --check` and `git diff --check` exited 0 at 08:06; the layout command exited 1 with seven reported directories. The parent attributes layout output to the checker consuming duplicate paths from unmerged index stages: `check_layout.py`'s `git_tracked_files` does not deduplicate the `git ls-files` output, and the later scan counts each path. No checker or index changes were made. Parent-observed physical touched directories have at most seven entries; largest touched source file is 606 lines and `route_services.rs` is 11 lines. This session ran no cargo check/clippy/build/test, rustc, or npm commands and performed no git mutation, manifest, or dependency edits. `NoWs` is crate-visible for default Context use and tests; routes remains public for WS hook access and login module remains private. No final acceptance report is recorded here while parent review continues.

- Progress: `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs` removed one duplicate tunables import; full source re-read, no other launcher edits in this correction.

### Revision 2 final status

- **Part A — source resolved:** all 12 conflict files are marker-free, with AUTH1 login and T6 WebSocket implementation retained. The touched source inventory is: `crates/shamir-server/src/boot/config/http_listener.rs`, `crates/shamir-server/src/boot/config/tests/http_listener_tests.rs`, `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs`, `crates/shamir-server/src/lib.rs`, `crates/shamir-server/src/security/mod.rs`, `crates/shamir-server/tests/e2e/http_rest/common.rs`, `crates/shamir-transport-http/src/lib.rs`, `crates/shamir-transport-http/src/server/accept.rs`, `crates/shamir-transport-http/src/server/mod.rs`, `crates/shamir-transport-http/src/server/routes/mod.rs`, `crates/shamir-transport-http/src/server/routes/route_services.rs`, `crates/shamir-transport-http/src/server/routes/ws/handler.rs`, `crates/shamir-transport-http/src/server/service.rs`, `crates/shamir-transport-http/src/server/tests/login_routing_tests.rs`, and `deploy/server.example.ktav`. Key source references: combined `Context` and route dispatch in transport service around lines 16 and 53; combined accept entry around line 147 and tracked upgrade handling around line 315; launcher combined route calls around lines 412 and 440, with login drain/cancel around lines 425–438; harness chmod around line 262; combined-route regression around line 242; WS authority alias/host validation in config around lines 6 and 149.
- **Part B — source resolved:** the test harness grants database mode `0o757` and keeps store mode `0o711`; server policy was not changed.
- **Part C — source review complete, pending CI:** parent reports full lint-aware read-through. Scoped formatting and `--check`, plus `git diff --check`, exited 0 at 08:10:27 CET. Layout remains PARTIAL: checker reports seven duplicate-stage directory counts while the parent observed physical touched directories at or below seven entries and source files at or below 606 lines. The new `route_services.rs` is 11 lines. The orchestrator must resolve the index (including that new file) and rerun layout. Compilation, clippy, tests, and behavioral CI remain pending; AC1–AC12 remain PARTIAL. This session ran no compile/test/rustc/npm command and made no git, manifest, or dependency changes. The index remains unmerged (UU/AA) and was not modified.

### Final response

Resolved source conflicts in all 12 files, retaining AUTH1 and WS; added RouteServices and combined-route coverage, and changed the native harness database mode to 0o757. Changes cover shamir-server, shamir-transport-http, deploy/server.example.ktav, and DESIGN.md.
Re-read every touched file. Scoped cargo fmt/--check and git diff --check pass; no compilation or tests were run.
Layout check remains PARTIAL: unmerged index stages are counted repeatedly; physical touched folders have at most 7 entries and sources at most 606 lines.
Remaining: orchestrator marks resolutions (including new route_services.rs), reruns layout, and runs CI. Merge/index left untouched; no children remain running.
