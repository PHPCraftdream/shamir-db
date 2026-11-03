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

## Current response summary

PARTIAL: REVIEW-2 N1–N10 source changes and additional coverage are present in shamir-server and shamir-transport-http; DESIGN.md is one current report. Formatting, layout, and whitespace checks pass; compilation/runtime and remaining coverage require CI. Two prohibited child compilation attempts were terminated without a successful build or test result. No commits, pushes, manifest/lock changes, moves, or deletions were performed.
