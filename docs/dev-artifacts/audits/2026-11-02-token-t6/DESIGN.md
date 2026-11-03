# T6 work item 1 — HTTP transport design

## Initial design

The transport owns HTTP routing, upgrade hand-off, response assembly, the Hyper upgrade boundary, and the guarded-I/O tunnel; it remains independent of WebSocket protocol semantics and token details. `WsUpgrade<G>` has three phases: `check_head(route, head, peer)` performs secret-free validation; native route authentication remains the `TokenGate` `Endpoint::Ws` phase; `admit(route, head_ok, grant, peer)` returns application session and extra response headers; `run(session, io, shutdown)` owns the upgraded lifetime. `UpgradeHead` exposes route path, method, and repeated header values. `UpgradedIo` is the transport-owned async byte stream wrapper, preventing Hyper types from entering server code.

`serve` remains source-compatible and uses `NoWs` (404); `serve_with_ws` accepts a hook. Upgrade ownership is handed from the request service to the existing per-connection task using a per-connection oneshot. The same task and admission permit remain live through `run`; no detached spawn is introduced. The application-owned response headers supplement transport-owned `Upgrade`, `Connection`, and status headers. The response body uses an explicit empty body so no unrelated response-body hold can interfere with upgraded protocol state.

The token revocation ordering for the later server implementation is publish-then-cancel: the revocation mirror is published with Release semantics before cancellation; connection registration precedes an Acquire recheck. Thus a concurrent revoke is observed either by the recheck or by cancellation. Item 1 establishes only the transport hook/lifecycle and does not implement server-side token registration.

The observed transport public items are `WsRoute`, `UpgradeHead`, `HeadOk`, `WsReject`, `UpgradedIo`, `WsUpgrade`, and `serve_with_ws`. The Hyper wrapper field is private; `UpgradedIo` accepts Hyper's upgrade `Parts` only inside transport code. No existing visibility has been widened beyond the newly exposed API. `UpgradedIo` is a simple wrapper around `TokioIo<hyper::upgrade::Upgraded>`; Hyper's `Upgraded` retains and replays buffered bytes. The connection task receives `OnUpgrade` through the service hand-off and passes it directly to the wrapper; no downcast or transport-I/O recovery is used.

## Progress

- Initial design recorded before implementation, including the publish-then-cancel / register-then-recheck ordering and API plan.
- Item 1 transport hook and `UpgradedIo` added; `serve_with_ws`, WS route dispatch, `101` response assembly, Hyper upgrade hand-off, JoinSet-owned run lifecycle, GuardedIo terminal tunnel state, and route/tunnel test modules added. Source review identified and corrected the missing-OnUpgrade response path so it cannot issue `101`. The implementation remains PARTIAL and uncompiled/unverified; no tests or commands were run in this continuation. The pre-101 tunnel test records the actual parked-read behavior and does not claim scanner rejection.

## Final report

Acceptance 1: PARTIAL — transport returns an application hook `101` with hook headers; token binding semantics remain for later items. Coverage: `ws_route_returns_hook_headers_and_holds_permit_through_session`; CI must compile/run it.
Acceptance 2: PARTIAL — native auth gate is called only after `check_head`; server-side exact protocol validation and refusal equivalence remain for later items. No dedicated negative-path tests yet; CI needed.
Acceptance 3: PARTIAL — server token rails/audit are not in item 1.
Acceptance 4: PARTIAL — browser path dispatch exists but no server browser policy/pre-auth accounting is assigned here. No dedicated test yet.
Acceptance 5: PARTIAL — first-frame auth is later work.
Acceptance 6: PARTIAL — token session loop is later work.
Acceptance 7: PARTIAL — token/SCRAM sid isolation is later work.
Acceptance 8: PARTIAL — token revocation and teardown are later work.
Acceptance 9: PARTIAL — tracked shutdown lifecycle is implemented, but close frames/heartbeat are later work. Coverage: `ws_route_returns_hook_headers_and_holds_permit_through_session`; CI needed.
Acceptance 10: PARTIAL — admission permit remains held through WS run; SCRAM accounting proof is later work. Coverage: `ws_route_returns_hook_headers_and-holds-permit-through-session`; CI needed.
Acceptance 11: PARTIAL — metrics/audit/redaction are later work.
Acceptance 12: PARTIAL — no changes made to SCRAM framing/loop behavior. CI must run existing transport tests and the new `tunnel_passes_arbitrary_binary_bytes_both_directions_after_101`, `bytes_before_101_remain_inside_the_http_scanner`, `serve_uses_no_ws_and_returns_not_found`, and `ws_route_returns_hook_headers_and_holds_permit_through_session`. The pre-101 test only checks the current parked-read behavior; it does not prove a rejection/scanner postcondition.

## Corrected implementation status (source read-through, 2026-11-02)

This section supersedes the earlier optimistic work-item status above. The parent reports scoped `cargo fmt --check` runs for all four touched crates and the layout checker passed at 22:00; formatting/layout success is not compilation, test, or behavioral verification. This audit pass ran no commands and no tests. All acceptance items 1–12 are therefore **PARTIAL** pending CI. Item 2 (raw WebSocket framing) has source changes and parent-reported formatting pass, but raw protocol behavior and SCRAM parity remain unverified. Item 4 (cancel registry/config integration) has source changes and was read, but has no compile/test verification. Item 3 classifier/gate/maintenance has source changes but is PARTIAL: the maintenance helper mismatch observed earlier was corrected by renaming the helper to `channel_label`; substantial rewritten worker behavior remains unverified.

### Actual APIs and visibility changes

- `RestGate::verify_browser(&self, token: &[u8], peer: PeerInfo) -> BrowserVerdict` and `RestGate::record_browser_failure(&self, peer: PeerInfo)` are `pub(crate)` methods in `crates/shamir-server/src/security/perimeter/http_rest/gate.rs`.
- `BrowserVerdict` (`Verified`, `Failed`, `ChannelDenied`, `PlaintextWriteDenied`) is `pub(crate)` and re-exported from `http_rest` for the WS route.
- `ScopedHandler::handle(&self, session, request, endpoint, conn)` and `HandleDenial` are `pub(crate)`; `HandleDenial` is re-exported from `http_rest` for `token_loop.rs`.
- Scope classifier takes `shamir_connect::server::token::Endpoint`; exhaustive `DbRequest` and `BatchOp` matches remain. REST transaction/cursor/subscribe rejection precedes read-only transaction checks; Execute remains read-only checked on both endpoints, while TxExecute/TxCommit checks are WS-specific.
- Maintenance records channel-tagged failures, retains a shared bounded subnet limiter, and stores independent REST/WS aggregates. Drop counters are separate; legacy `failure_drops` remains the REST counter used by existing tests. REST exposure details remain the original `(id, owner)` tuple; WS exposure details add `("ws", id, owner)`.

### Source findings and unverified blockers

- `crates/shamir-server/src/security/perimeter/http_ws/route.rs:94` assigns `peer.addr = resolved.addr`, while `:93-107` treats resolver output as a socket address. Parent reports `resolved SocketAddr.addr` as invalid; compile blocker requiring owner fix.
- `crates/shamir-server/src/sync/connection/token_ws/binding.rs:20` calls `expires_at_ns()` as a method. Parent reports the source field is not a method; compile blocker.
- `crates/shamir-server/src/sync/connection/token_ws/token_loop.rs:131` moves `ctx.grant` into an `Arc`, but later code borrows `ctx.grant` at `:195`, `:223`, `:282`, `:315`; `Arc::clone(&ctx.grant)` at `:270` also references the wrong expression. Compile blockers.
- `crates/shamir-server/src/sync/connection/token_ws/token_loop.rs:244` decodes as `rmpv::Value`; parent reports no direct `rmpv` dependency. Do not add a dependency without approval; use already available APIs or request a decision.
- `crates/shamir-server/src/sync/durability/tx_registry.rs:314-317` and `:322-325` define duplicate `remove_for_session` methods. The helper compares `owner_sid()`; parent reports the correct ownership field is `owner_user_id`. Compile blocker until reconciled with the transaction contract.
- `crates/shamir-server/src/security/perimeter/http_ws/route.rs:131-160, 163-254` needs ownership/lifetime review around grant transfer and activation. The route/loop handoff still has the reported moved-grant issues.
- Parent reports `crates/shamir-transport-http/src/server/service.rs` uses immutable request extensions where WS routing needs mutable extensions; cross-owner compile blocker, not edited here.
- Parent reports the pre-101 transport test aborts the connection and lacks an oracle proving scanner rejection; transport acceptance remains partial.
- `crates/shamir-server/src/security/perimeter/http_ws/` has eight direct entries (`browser_policy.rs`, `cancel_registry.rs`, `first_frame.rs`, `mod.rs`, `pre_auth.rs`, `route.rs`, `tests/`, `upgrade.rs`), exceeding the brief’s seven-entry limit. Layout checker ignores untracked files; this manual layout issue remains.

### Actual item-3 tests and limitations

`crates/shamir-server/src/security/perimeter/http_rest/tests/scope_tests.rs` contains WS cases `ws_tx_and_subscription_are_allowed_only_on_ws`, `ws_scope_classifies_all_db_requests_deny_by_default`, and `ws_readonly_tx_rules_and_database_allowlist_are_enforced`, plus REST compatibility cases. The mixed-array scope test uses an index split for allowed cases; this is brittle and is not evidence of exhaustive classification. It should use explicit expected decisions per request before claiming exhaustive coverage.

`crates/shamir-server/src/security/perimeter/http_rest/tests/gate_tests.rs` adds `native_ws_auth_accepts_ws_token_and_rejects_rest_channel`, `browser_verification_never_rate_limits_valid_token_and_records_frame_failures`, and `browser_verification_reports_channel_and_plaintext_rails`. These cover valid WS, channel denial, a burst of frame failures followed by a valid token, and rails at source level, but were not run and do not establish all browser failure/limiter boundary properties. Existing REST audit summary expects `rest`; existing maintenance assertions for summary logs and REST exposure tuple `(id, owner)` were retained. No complete regression proof is available.

### Corrected acceptance report — all PARTIAL, CI required

Acceptance 1: PARTIAL — candidate source `crates/shamir-server/src/security/perimeter/http_ws/route.rs` and `crates/shamir-server/src/sync/connection/token_ws/binding.rs`; binding/deadline wire oracle not verified. CI compile and WS tests required.
Acceptance 2: PARTIAL — candidate source `http_ws/upgrade.rs` and `http_rest/gate.rs`; exact refusal equivalence, caps, and Origin ordering not established. Gate test `native_ws_auth_accepts_ws_token_and_rejects_rest_channel` covers only selected outcomes; CI required.
Acceptance 3: PARTIAL — gate rail source exists; complete plaintext audit/channel behavior not verified. CI required.
Acceptance 4: PARTIAL — candidate source `http_ws/{upgrade.rs,pre_auth.rs,route.rs}`; route availability, caps, and permit release are not fully verified. CI required.
Acceptance 5: PARTIAL — candidate `http_ws/first_frame.rs`; first-frame defect uniformity, close behavior, and permit release are unverified. CI required.
Acceptance 6: PARTIAL — candidate `sync/connection/token_ws/token_loop.rs`; request/reply, scope errors, subscriptions, cursors, and DB allowlist are unverified. CI required.
Acceptance 7: PARTIAL — candidate sid check in `token_loop.rs:257-265`; cross-protocol isolation is unverified. CI required.
Acceptance 8: PARTIAL — candidate `http_ws/cancel_registry.rs`, `route.rs`, `token_ws/teardown.rs`; revocation, owner invalidation, rollback, cursor cleanup, cancellation, and registry lifecycle are unverified. Duplicate tx-registry helper is a blocker. CI required.
Acceptance 9: PARTIAL — candidate `token_ws/{token_loop.rs,framer.rs}`; close codes, heartbeat, shutdown, and frame limits unverified. CI required.
Acceptance 10: PARTIAL — session separation appears in route construction; SCRAM-cap accounting not verified. CI required.
Acceptance 11: PARTIAL — metrics/audit source includes endpoint labels and REST-compatible exposure shape; redaction/channel aggregation unverified. CI required.
Acceptance 12: PARTIAL — raw framing changes are confined to transport-ws framing by source review; SCRAM parity and tests unverified. CI required.

Progress item 2 (raw framing): source reviewed; parent reports scoped formatting passed; functionality and tests unverified. Progress item 4 (cancel registry/config): source read; parent reports wiring present; compile/runtime behavior unverified. The note does not claim that this audit continuation ran commands; the prior incident recorded above remains intact. Parent must resolve source blockers and obtain CI results before changing any acceptance status.

## Errata (2026-11-02; supersedes inaccurate statements above)

This errata preserves the historical audit text but corrects its factual claims. Source blockers listed here must be repaired before CI can establish acceptance; no code or tests were changed or run in this documentation-only pass.

- Formatting: parent reports HTTP and WS formatting checks passed at 22:00, but the server formatting check failed while a worker was editing `token_loop` imports. A chained command returned success because a later `date` command masked the failure. Do not describe all four crates as formatted. The layout checker passed, but manual inspection found eight direct entries under `crates/shamir-server/src/security/perimeter/http_ws/`, exceeding the seven-entry maximum.
- REST exposure: parent’s git-diff review confirms the maintenance test assertion was changed from `("rest", id, owner)` to `(id, owner)`. This violates acceptance 12’s requirement not to change assertions; do not claim the original REST tuple was preserved. The gate test’s REST audit expectation remains `rest`.
- Transaction registry: `owner_sid` is the correct session teardown key. The defect is duplicate `remove_for_session` definitions in `crates/shamir-server/src/sync/durability/tx_registry.rs:153,158,163` (as reported from parent review), where the replacement accidentally displaced the original `owner_user_id() -> &[u8; 16]` helper. Do not recommend replacing the teardown key with `owner_user_id`.
- `rmpv`: its uses in `crates/shamir-server/src/sync/connection/token_ws/tests.rs:22,59` are in tests, not `token_loop.rs:244`; that production use is separately present. The claim that `rmpv` is absent as a direct dependency is parent-reported and was not independently verified here.
- Routing/integration blockers reported or source-observed: `http_ws/mod.rs` does not declare `mod upgrade;` despite the route import; the launcher’s `WsUpgradeService` literal lacks `gate`; `route.rs` accesses invalid `resolved.addr` on a `SocketAddr`, moves fields from `Drop` type `WsSession`, and needs the `SinkExt` import for `socket.send`. `BrowserOriginPolicy` is passed as a `Vec<String>` (`route.rs`) but expected as a policy type (`upgrade.rs`), so the policy is not integrated. These are source blockers, not issues to defer to CI.
- `crates/shamir-server/src/security/perimeter/http_ws/mod.rs` also has an ungated `mod tests;`, pulling test imports into production compilation. Review and correct before compile verification.
- `first_frame.rs:73` uses `rmp_serde::from_slice`. The user reports inspection of `rmp-serde 1.3.1` `decode.rs:1171–1187` confirms this `Deserialize` path does not enforce end-of-input; valid token data followed by trailing bytes may be accepted. This external source was not independently fetched during this pass.
- Pre-auth limiter race (inference, not a runtime observation): `pre_auth.rs:36–47` clones the counter under a DashMap entry guard and increments after releasing it; `Drop` decrements and removes the map entry at `:68–78`. An interleaving may remove the old entry before the new acquirer increments its cloned counter, allowing a fresh counter and potentially exceeding the per-subnet limit. No runtime reproduction was performed.
- `browser_policy.rs:39–96` rejects uppercase hostnames and default ports, whereas `policy_tests.rs:14–20` expects canonicalization. The pre-101 transport test aborts the connection without an oracle proving scanner rejection. Both remain behavioral/test gaps.
- The `ws_scope_classifies_all_db_requests_deny_by_default` test uses an index split for allowed cases; its brittle structure does not establish exhaustive classification.

## Final report

Acceptance 1: PARTIAL — route/binding candidate source exists, but routing has the blockers above; no compile or behavior verification.
Acceptance 2: PARTIAL — gate/auth coverage is incomplete; the origin-policy type mismatch and refusal-equivalence gaps remain. `native_ws_auth_accepts_ws_token_and_rejects_rest_channel` was not run.
Acceptance 3: PARTIAL — rails exist in source; channel/plaintext behavior and audit coverage are unverified. `browser_verification_reports_channel_and_plaintext_rails` was not run.
Acceptance 4: PARTIAL — pre-auth accounting and route integration have unresolved source concerns; no cap/permit-release verification.
Acceptance 5: PARTIAL — first-frame handling has the trailing-byte concern; uniform failure/close behavior unverified.
Acceptance 6: PARTIAL — token loop and request/reply behavior are unverified; source blockers remain.
Acceptance 7: PARTIAL — sid isolation candidate exists; cross-protocol behavior unverified.
Acceptance 8: PARTIAL — cancellation/teardown candidate exists; duplicate registry helper remains a source blocker.
Acceptance 9: PARTIAL — framing/lifecycle candidate exists; close, heartbeat, shutdown and frame-limit tests unverified.
Acceptance 10: PARTIAL — session separation candidate exists; SCRAM-cap accounting unverified.
Acceptance 11: PARTIAL — audit/metrics source exists; REST assertion was changed contrary to acceptance 12, and redaction/channel aggregation are unverified.

### Correction note (2026-11-03)

This note corrects the preceding progress claims without rewriting the historical text. The statement at line 155 that foreign/zero/SCRAM sid-to-token cases return HTTP 401 with zero registration is incorrect: the source closes the WebSocket with code 4010, not HTTP 401; zero registration is not asserted by an oracle, and outer registration after close remains a lifecycle residual. The shutdown statement at line 157 that parent reread was pending is stale: parent has reread the full 16-count/Pong and 1000-close source. The raw TLS + SCRAM helper has been added and parent reread lines 124–294 and call site 367; it trusts the fixture CA and TLS exporter, and the token SID-to-ErrorEnvelope `session_expired` case is exact. The imports/header issue was fixed. Native read-only follow-up ping correctly uses `read_sid`, not a spare SID; parent verified lines 333–351. The oversize end-to-end case tests only a 14-byte masked final binary frame header declaring 16 MiB + 1, with no payload, and expects exact close 1009; it does not test transmission of a full oversized payload. Parent's latest formatting checks for all four crates, layout check, and git-diff check exited 0 at 00:31; subsequent instrumentation/test changes still require final formatting check. The pre-auth race exercise ran 64 iterations: the old behavior showed one permit followed by a last drop, not eight drops; nondeterministic scheduling is not proof. These are source-review/check reports only: all acceptance items remain PARTIAL, with no compilation, runtime, or test-pass claim. No final report is recorded here.

Revision 1 started at 23:11 CET on November 2. Implementation stops at 01:00 CET on November 3, followed by a 01:00–01:30 CET review window. Three disjoint edit-only workers for WP-A, WP-B, and WP-C are running. No compilation or tests are allowed; only the orchestrator may run formatting/layout checks. All acceptance items remain PARTIAL pending CI.

The stale blocker lists and prior status claims above are historical, not current evidence; REVIEW-1 is the current baseline for defects, acceptance status, and the WP-A/B/C work plan. REVIEW-1 step 0 moves are completed and must not be repeated. No fixes are claimed in this start note.

- Revision-1 WP-A initial pass PARTIAL: transport imports/helper extraction, tunnel test oracles and duplicate test attribute edited; handoff/read-through/route matrix still under follow-up. No commands/tests run by worker.
- Revision-1 WP-B initial pass PARTIAL: browser parsing/policy/pre-auth race and activation/launcher source edits made; REST maintenance/test restoration and acceptance coverage remain in progress. Saved source still requires orchestrator read-through; no commands/tests run.
- Revision-1 WP-C core pass PARTIAL: token loop ownership/import/teardown changes, registry duplicates, binding decode helper and shared harness visibility edited; framer/e2e acceptance coverage remains in progress. No commands/tests run by worker.

These are worker claims pending parent full reread, not runtime evidence. No item is claimed DONE.

- Revision-1 WP-A follow-up: parent reread transport hook/handler, accept handoff, service, tunnel and route tests; repaired duplicate imports and restored Send-bounded trait signature. Parent ran cargo fmt and cargo fmt --check scoped to shamir-transport-http/shamir-transport-ws/shamir-connect successfully; layout checker returned OK. Runtime/compilation remains unverified; transport test readers still require bounded-read hardening.
- Revision-1 WP-B/C follow-ups: new browser parser/pre-auth/policy, WS redaction/exposure, framer heartbeat/size, native/revocation e2e source added. Parent source review found and delegated repairs for invalid Rust, incorrect size oracles, private imports, audit tuple mismatches and race-prone tests; not all repaired files have completed final reread. All acceptance statuses remain PARTIAL pending CI and remaining work.

## Revision 1 superseding progress (2026-11-03 00:10 CET)

Parent source reread confirmed metrics use zero-registration REST constructors and restored REST-only labels. WS preregistration moved to the launcher, and the missing `PreAuthAdmission` import in `launch.rs` was corrected. Parent reports `cargo fmt --check` scoped to `shamir-transport-http`, `shamir-transport-ws`, and `shamir-connect` exited 0, and `python -I scripts/ci/check_layout.py` exited 0 (`layoutOK(0baseline/0hints)`). These checks are not compilation, tests, or runtime verification.

Parent counted untracked-containing folders: `http_ws` 6, `browser` 4, `http_ws/tests` 5, `token_ws` 6, `token_ws/tests` 3, and `e2e/http_ws` 7. Server formatting has not yet run, pending native/revocation edit workers. Remaining source follow-up: native `ResponseEnvelope` nonexistent `error` field, test seed ordering, cursor/push handling and limiter matrix; revocation missing `SinkExt` and the snapshot `Event.commit_version == 0` oracle.

Lifecycle residual: token-store/transaction/cursor/gauge cleanup precedes writer finish, but cancellation-registry registration remains owned by outer `WsSession` until `run` returns, potentially after the close frame; acceptance 8 remains PARTIAL. No acceptance item is DONE, and no compilation or runtime behavior is claimed verified.

## Revision 1 current progress (around 00:21 CET)

Parent full reread found `tx_registry::remove_for_session` had been removed during duplicate cleanup. A single sid-lookup method was restored, dropping the guard before `self.remove`; the original `expired_handles` documentation was also restored.

Revocation e2e source was reread and repaired: `SinkExt` import, snapshot `commit_version == 0` acceptance with live event version greater than zero, typed payload key/value/version assertions, retention of ready state through the live-event wait, an overall readiness deadline, and expected `revoke_user == 1` after changing the trigger to native admin. A WS failure-summary test was added for serialized channel `ws`, result `invalid`, total 1, and subnet sample; the existing WS exposure tuple coverage remains.

Parent reports transport formatting, formatting checks and layout checks passed at approximately 00:16; server formatting has not yet run while awaiting the native worker. The first native worker report was partial. Parent review found further source breakage in recovery, including incorrect wire key `request_id` instead of `rid`, denial field `Vec.res`, missing table-create execution, and duplicated ping handling. Narrow repairs and native case coherence are delegated/in progress. All acceptance items remain PARTIAL: no acceptance is DONE, and no compilation, tests, or runtime behavior are claimed verified.

Only this progress note is current; prior blocker/status text is historical and may have been superseded by the repairs noted here.

## Revision 1 current progress (current)

The initial server formatting attempt failed at `route.rs:314` due to a missing parenthesis. After repair, the retry exited 0; scoped formatting checks for all four crates and the layout check passed. No compilation, tests, `cargo check`, or clippy were run. All acceptance items remain PARTIAL pending CI.

`Cargo.lock` pins Hyper 1.9.0, Tokio 1.51.6, Tokio/Tungstenite 0.30.0, rmp-serde 1.3.1, and scc 3.8.4. The local registry has older versions only; exact 0.30.0 semantics remain unverified.

Source-only coverage added or repaired, without runtime proof:
- Native 401 matrix for canonical unknown/wrong secrets isolated behind trusted proxy /24; malformed heads return 400 and remaining failures 403. Token cap is 2 and owner cap is 3.
- Foreign/zero/SCRAM sid-to-token cases return 401 with zero registration. Raw token SID-to-SCRAM helper was added; parent reread of imports/repair is pending.
- Read-only follow-up ping uses `read_sid` correctly. Native read/write, transaction, cursor, admin/database denials, actual 16 MiB limit, and text-close cases are covered in source.
- Shutdown grouping covers store count 16 and 16 Pongs while SCRAM admin remains active, then 1000 each; parent reread is pending.
- Browser canonical-credential cases cover 4000 and binding consumed before expiry wait, then 4012.

Registry registration outside the outer close boundary remains a lifecycle residual. All acceptance items remain PARTIAL; source-only changes are not runtime proof and await parent reread and CI.

### Final pre-review checkpoint (2026-11-03)

Correction to prior progress: SID isolation assertions use WebSocket close 4010, not HTTP 401; registry-entry removal is not asserted by these e2e cases. The outer WsSession registration is still held until run_token_loop returns, potentially after its close frame. All acceptance items remain PARTIAL pending CI.

The parent reread the raw TLS/SCRAM helper and its call, the corrected read_sid follow-up ping, and the 16-active-token/SCRAM-login shutdown scenario. The oversize e2e case sends only a 14-byte masked final-binary header declaring 16 MiB + 1, without a payload, and requires 1009. No full oversized upload was tested. The pre-auth race test now races dropping its only old permit against registration for 64 iterations; scheduling is not deterministic. The first-frame absolute-deadline test checks task completion at six seconds before awaiting, so auto-advance cannot hide a reset deadline.



## Final report — revision 1

### Observed gates and source changes

Parent-reported latest gate sequence at 00:38, chained with `&&` and exiting 0: `cargo fmt -p shamir-server` (after the parse fix and late edits); `cargo fmt -p shamir-server -p shamir-transport-http -p shamir-transport-ws -p shamir-connect -- --check`; `python -I scripts/ci/check_layout.py` (`layoutOK`, 0 baseline, 0 hints); and `git diff --check`. No `cargo check`, clippy, build, test, rustc, npm, or benchmark was run; runtime behavior remains unknown. Read-only diff against `origin/master` confirms REST scope/executor/maintenance and SCRAM framer existing assertions/semantics retained, signature updates only, plus additive WS tests. No git mutations, manifest/lockfile/dependency changes, file deletions, or moves. No files requiring deletion were found; parent deleted none.

Manually counted directories: `http_ws` 6, `browser` 4, `http_ws/tests` 5, `token_ws` 6, `token_ws/tests` 3, `e2e/http_ws` 7, transport routes 2, transport WS 3. Counts include untracked files. New/changed source files are below 1,000 lines: e2e native 683, session loop 379, revocation 574, browser 390, shutdown 115. `Cargo.lock` pins hyper 1.9, tokio 1.51.6, tokio/tungstenite 0.30, rmp-serde 1.3.1, scc 3.8.4; exact 0.30 registry sources are absent, so API semantics are unverified.

Observed visibility widenings: transport service `auth_header`, `retry`, `unauthorized_body` are pub(crate); `ScopedHandler::handle` and `HandleDenial` crate-visible; `encode_prereserved`/`ErrorReply` pub(super) for tests; `ADMIN_PASSWORD`/`grant_default_main_access` pub(crate) for test harness. `BoxedIo`, `resolved_peer` remain private and REST public re-exports were restored. Supervised writer JoinHandles and instrumented dispatch are recorded deviations; bare spawn remains bounded by RAII.

### Packages

| Package/work package | Status | Source evidence and remaining verification |
|---|---|---|
| WP-A — `shamir-transport-http` / `shamir-transport-ws` | PARTIAL | `crates/shamir-transport-http/src/server/accept.rs:53,79,244`, `server/service.rs:44`, `server/routes/ws/handler.rs:12`, `framing/guarded_io.rs:373`; route and tunnel tests in `server/tests/ws_route_tests.rs:180+` and tunnel tests `5+`. Raw-framing tests retain existing prefix assertions; mirror-only `shamir-connect` documentation restoration has no semantics. CI compile/runtime and full wire verification required. |
| WP-B — server perimeter, browser, launcher, maintenance | PARTIAL | `http_ws/route.rs:86,149,261`, `browser/policy.rs:39`, `http_ws/pre_auth.rs:36`, `http_ws/first_frame.rs:15`, launcher `:155`, maintenance `:647`; native plaintext and WS audit/redaction coverage in gate tests. CI required. |
| WP-C — token loop/framer, binding, registries, e2e | PARTIAL | `token_ws/token_loop.rs:125,302,397`, `framer.rs:49,83`, `binding.rs:7,15,26`, `tx_registry.rs:311`, `cursor_registry.rs:567`, shared writer `session/writer_task.rs:11`, request loop `request_loop.rs:215`; four e2e launch groups. CI required. |

### Acceptance

| # | Status | Source evidence, coverage and residual |
|---|---|---|
| 1 | PARTIAL | Exact binding keys, nonzero sid and expiry/deadline covered by `binding_tests.rs:17` and `session_loop.rs:32,296`; compilation and wire oracle require CI. |
| 2 | PARTIAL | Canonical uniform 401 matrix and malformed/unknown/wrong-secret/revoked/expired cases in native tests `:68`; differentiated XFF/24 trusted-proxy limiter avoids contamination; malformed head 400, REST-only 403, bad method/Origin 403; caps 2/token and 3/owner; limiter last at `:633`. CI required. |
| 3 | PARTIAL | Native plaintext 403 and read-only allow with exposure tuple; WS gate/native plaintext tests and browser-rail verdict unit coverage. No full-wire remote-plaintext WS test. CI required. |
| 4 | PARTIAL | Browser Origin/Host matrix and policy tests; global 256/subnet 8 unit coverage (`pre_auth_tests.rs:11,24`); real first-frame absence covered browser e2e `:80`. Missing remote-plaintext browser 404 wire test, full-wire/pre-auth 503, and all-route permit-release oracle. |
| 5 | PARTIAL | First-frame matrix `first_frame_tests.rs:43,52,75,92,109,126`; canonical browser credential helper `:160`; all invalid cases same fixed 4000 reason, REST-only 4013, busy 1013, plaintext 4014 unit cases. Full-route permit release for all failures unproven; CI required. |
| 6 | PARTIAL | Native RW Execute/Tx/cursors/read-only/admin/database cases and exact success-reply keys helper `session_loop.rs:82`; subscription ready/event coverage `:166,302`. No full-wire `{rid,error}` `rate_limited`/`invalid_request` oracle. CI required. |
| 7 | PARTIAL | Foreign token and zero SID native `:164`; SCRAM-to-token native `:331`; token-to-SCRAM session `:124`, exact `session_expired` ErrorEnvelope. CI required; store separation is shown behaviorally, not by direct SCRAM-store assertion. |
| 8 | PARTIAL | Revocation `:277`: Ready then live event before revoke, 4011/no post-push, `revoke_user` 1, role-change 4011, TTL 2 seconds -> 4012, staged 987654 not visible, cursor not found; cancel-registry last-drop unit test. Residual: registration remains owned by outer `WsSession` until `run` returns after writer close (`route.rs:60,232`); fixed context contract was not changed to handoff. Registry entry removal before close is not asserted. |
| 9 | PARTIAL | Text 1003; oversize 1009 test sends only a 14-byte masked header declaring 16 MiB+1, not the payload; heartbeat unit (`framer.rs:105`) close 4003; shutdown 16 sockets, 1000 and no report errors. CI including exact 0.30 capacity/close behavior required. |
| 10 | PARTIAL | Shutdown test verifies configured 16 tokens, store count 16 and all 16 Pongs while SCRAM admin login/ping active; fixed SCRAM cap 16 at `session_loop.rs:427`. CI required. |
| 11 | PARTIAL | WS metrics labels at launcher `:155`, request denied/duration at token loop `:352,367`, gauge teardown `:38,52`; WS failure/exposure audit tests maintenance `:684,710`; redaction gate-level `:449` success/failure for both routes. Active-gauge recorder oracle and full-route audit/redaction/frame-handler log capture missing. CI required. |
| 12 | PARTIAL | REST/SCRAM master assertions and semantics retained by source review; writer extraction and raw WS share control/heartbeat paths. All regressions must run in CI. |

### Residuals

Observed source lifecycle gap: cancellation-registry registration remains held by outer `WsSession` until the run returns after writer close; removal before close is not asserted. Inferred, unverified risks include writer close behavior after capacity limits, scheduler abort, and OS-specific behavior. No runtime failure is confirmed. Formatting and parsing/source review do not establish compilation; no stale compile-error claim is made as fixed beyond the syntax parse/format gate. Exact tungstenite 0.30 behavior remains unverified. Coverage gaps are listed per acceptance above.

### Final user response

PARTIAL: revision-1 transport, perimeter, loop, and acceptance-test source repairs are in the worktree, with formatting/layout/whitespace checks passing. Compilation and tests were not run, as requested. Cancellation-registry removal before the close frame and the remaining coverage gaps still need resolution/CI; no commits, pushes, manifest changes, moves, or file deletions were performed.

### Final report corrections and exact source paths

**Corrected package evidence**

- WP-A accept path: `crates/shamir-transport-http/src/server/accept.rs:53,79,244` (not `server/routes/ws/accept.rs`).
- WP-B browser pre-auth and first-frame paths: `crates/shamir-server/src/security/perimeter/http_ws/browser/pre_auth.rs:36` and `crates/shamir-server/src/security/perimeter/http_ws/browser/first_frame.rs:15`.

**Corrected acceptance evidence**

- AC2: bad method is 405 with `Allow: GET`; bad Origin is 403. These are distinct outcomes, not both 403. Prior line 155's observed source facts: malformed heads 400, REST-only/channel access 403, bad method 405, bad Origin 403, canonical credential failure 401. The source evidence does not establish remaining runtime behavior.
- AC6: subscription-ready/event test references are `crates/shamir-server/tests/e2e/http_ws/revocation.rs:166,302`, not `session_loop.rs`.
- AC9: heartbeat unit test is `crates/shamir-server/src/sync/connection/token_ws/tests/framer_tests.rs:105`, not the production framer.
- AC10: fixed cap source is `crates/shamir-connect/src/server/state/session.rs:427`, not `session_loop.rs`.
- AC11: audit tests are in `crates/shamir-server/src/security/perimeter/http_rest/tests/maintenance_tests.rs`, functions `ws_exposure_audit_summary_carries_ws_channel` and `ws_failure_audit_summary_carries_ws_channel`. No line numbers are asserted here.

**Path prefixes for abbreviated AC references above**

- `native` = `crates/shamir-server/tests/e2e/http_ws/native.rs`
- `browser` = `crates/shamir-server/tests/e2e/http_ws/browser.rs`
- `session_loop` = `crates/shamir-server/tests/e2e/http_ws/session_loop.rs`
- `revocation` = `crates/shamir-server/tests/e2e/http_ws/revocation.rs`
- `shutdown` = `crates/shamir-server/tests/e2e/http_ws/shutdown.rs`
- `token_loop` = `crates/shamir-server/src/sync/connection/token_ws/token_loop.rs`
- `route` = `crates/shamir-server/src/security/perimeter/http_ws/route.rs`
- `launcher` = `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs`
- `maintenance` = `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs`
- `binding_tests` = `crates/shamir-server/src/sync/connection/token_ws/tests/binding_tests.rs`
- `first_frame_tests` = `crates/shamir-server/src/security/perimeter/http_ws/tests/first_frame_tests.rs`
- `pre_auth_tests` = `crates/shamir-server/src/security/perimeter/http_ws/tests/pre_auth_tests.rs`
- `gate_tests` = `crates/shamir-server/src/security/perimeter/http_rest/tests/gate_tests.rs`
- `redaction_tests` = `crates/shamir-server/src/security/perimeter/http_rest/tests/redaction_tests.rs`

### Continued-work correction (2026-11-03)

`native.rs:575` covers full 16 MiB + 1 payload and header-only 1009; `native.rs:587` covers malformed outer exact error and `:613` inner 91 invalid-request exact keys. These are source-only and unverified by CI; AC6 and AC9 remain PARTIAL.

The browser 8-socket timing-dependent test's new helper was removed; existing tests are unchanged. AC4 remains PARTIAL. Cancellation before close remains unresolved: fixed context has no ownership slot and requires approval for a contract change. Parent observed full scoped format check, layout, and diff check passing before this document append; final whitespace will be rechecked.

### Final user response

PARTIAL: native acceptance source now also covers full oversized payloads and exact invalid-envelope/invalid-request wire shapes. Formatting/layout/whitespace checks passed; compilation and tests were not run. Remaining lifecycle work needs approval to extend the fixed loop contract so cancellation registration can be owned and released by teardown before close; remaining coverage still needs CI. No commits, pushes, manifest changes, moves, or file deletions were performed.

## Approved lifecycle contract extension — follow-up

The approved deviation extends `TokenLoopContext` with one required field, `registration: TokenWsRegistration`; all other fields and signatures remain unchanged. The route takes the required `TokenWsRegistration` from an `Option<TokenWsRegistration>` using `.take().expect(...)` and puts that registration in the context, whose field is non-optional. The token loop transfers it to `TokenSessionTeardown` before writer spawn. Cleanup takes `&mut self`, drops the registration synchronously after dispatch abort/join, then signals `cleanup_complete` before writer finish.

A token-only `TokenWsCloseGuard` wraps the close path: it signals `close_requested` on close/shutdown, waits for `cleanup_complete`, delegates to the underlying close, and captures heartbeat close 4003 without changing the shared SCRAM writer. The reader selects on `close_requested` while parked, reading, or sending malformed-input responses. Early route activation failure/cancellation removes the store entry and registration before direct close.

New source test `crates/shamir-server/src/sync/connection/token_ws/tests/teardown_tests.rs` asserts writer close waits for registration cleanup: entry present and no early close, then registration dropped, acknowledgement observed, and close 4003. This is source-level coverage; exact full-pipeline coverage remains limited.

Additional source/read-through corrections: the browser-expiry fixture uses TTL 1, verifies the created record view, waits with a bounded 15-second window, then attempts the candidate token string and expects 4000. Maintenance 493 WS+REST exposure drops are represented by the sum. Previously alleged reviewer issues were refuted: the REST master tuple is `(\"rest\", id, owner)` and the channel must not be removed; created token-view IDs are public identifiers, not secrets; canonical config entries are intentional; MessagePack wide-string forms are allowed; and `expect_ready_and_event` is called once, at line 356, not dead code.

All WP-A/B/C and AC1–12 remain PARTIAL pending CI. AC8's earlier source gap is addressed by this handoff, but runtime ordering and complete coverage are not verified. Parent source reread reports no newly confirmed compile issue after fixes for the context partial move, missing `StreamExt`, manual `async fn`, and path segments. Parent formatting/layout gates for the new handoff have not yet been rerun and are pending. No compilation, tests, or runtime verification are claimed.

Production behavior remains uncertain around automatic Tungstenite close flushing and reader/writer interaction: the cleanup barrier has a two-second timeout, while writer join/write blocking may use a five-second abort path. This is an unverified risk, not a confirmed runtime bug.

## Incident report — unauthorized compilation attempt (2026-11-03, approximately 01:36 CET)

A child session (`5249ff1a`) initiated the forbidden command `cargo test -p shamir-server writer_close_waits_for_loop_cleanup_barrier` in async job `call_364f12da601d449abdbb4efea69a7109`, despite the user's prohibition on local compilation. The parent reviewed the delegation transcript and confirmed the violation. Parent job cancellation could not access the child-owned job; the available inject/stop tools rejected the request because the child session ID was missing. The parent identified cargo PID 38236 by its exact test command line and successfully terminated PID 38236 and its descendant processes 43316 and 43284. No source was reverted or deleted.

The child reported unrelated compilation errors; these were not independently verified and are not accepted as a diagnosis. Compilation was attempted without authorization; no successful compilation or test verification is claimed, and the test did not pass. This incident supersedes earlier statements that no compilation or tests had been run. The approved final gates—scoped formatting check, layout check (`layoutOK`, 0 baseline, 0 hints), and `git diff --check`—were subsequently reported by the parent to have passed with exit status 0 at approximately 01:36. These checks do not verify compilation or runtime behavior. WP-A, WP-B, and WP-C and AC1–12 remain PARTIAL pending CI; AC8's source-level cancellation-registration handoff is addressed, but runtime ordering and complete coverage remain unverified. Parent final source review is planned for 01:45–02:15. No commits, pushes, manifest changes, moves, or file deletions were performed.

### Final user response

PARTIAL: approved cancellation-registration handoff and token-only cleanup-before-close barrier are implemented in source, with regression coverage added. Compilation/runtime remain unverified. A child violated the no-compilation rule by launching `cargo test`; the identified process tree was terminated. Remaining coverage and CI validation are outstanding; no commits, pushes, manifest changes, moves, or file deletions were performed.

## Final review — 2026-11-03

### Parent final source review

Last source edits were completed by 01:43; the review-only cutoff was 01:45 and the hard cutoff 02:15. The parent reviewed the full current lifecycle files and diffs for transport, REST, launchers, and tests. The master diff confirms REST scope/executor/maintenance assertions were preserved with signature updates, SCRAM framer assertions are additive, and the shared writer body preserves heartbeat behavior, arm ordering, and comments.

Approved lifecycle ownership is present in source: route registration at `route.rs:235-252`; the required token-loop context field; teardown construction at `token_loop.rs:166-173` before writer spawn at `:177`; close-request selection at `:213,242,280`; and teardown after dispatch abort/join at `:411-415`, acknowledgement at `:416`, before `writer.finish` at `:420`. Teardown takes registration at `teardown.rs:48`; the close guard is at `:43-45,57-59`. Teardown tests at `tests/teardown_tests.rs:15-63` exercise the isolated barrier microprotocol with the actual registry, not full route/handler teardown integration.

Route failure branches now drop the pre-auth permit before close at `route.rs:162,176,183,190,198`; activation failure/cancellation drops registration and store entry before direct close. Browser absolute-expiry fixture is at `browser.rs:185-225`; native payload and error cases are at `native.rs:575-643`. Unused `AsyncRead`/`AsyncWrite` imports were removed based on the observed forbidden-build-log warning; no additional compile was performed.

Independent review findings identified as false positives were refuted and explicitly recorded above. Genuine remaining gaps are: remote-plaintext browser 404 wire behavior; full-wire pre-auth caps and all-terminal counter oracle; exact `rate_limited` wire keys; active gauge recorder and full-route audit redaction; and full-pipeline registry absence after each close.

No new confirmed compile error was found by read-only review. This is not a compilation claim. The unauthorized child `cargo test` incident is recorded above: its transcript confirms unused-import warnings and interrupted/missing-crate E0463 errors. Because the process was terminated, unrelated existing-error root cause cannot be claimed. The test-name filter was stale (`writer_close_waits_for_loop_cleanup_barrier` versus actual `writer_close_waits_for_registration_cleanup`), so even a completed run could have selected zero targeted tests; there is no runtime verification.

Observed approved gates exited 0 at 01:43:43: `cargo fmt -p shamir-transport-http`, scoped formatting checks for server/HTTP/WS/connect, layout (`python -I scripts/ci/check_layout.py`, baseline 0, hints 0), and `git diff --check`. Parent ran no additional tests, check, clippy, or build. Six untracked files were manually counted: token_ws 7 entries/tests 4, close guard 61 lines, teardown tests 63, native 760, browser 394; other untracked first-frame 171, pre-auth 62, framer 150, handler 181. No manifest/lock changes or deletions were present in git diff. No commits or pushes were made.

### Final package and acceptance status

| Package | Status | Remaining verification |
|---|---|---|
| WP-A — transport HTTP/WS | PARTIAL | CI compilation/runtime validation and wire coverage. |
| WP-B — server perimeter/browser/launcher/maintenance | PARTIAL | CI plus the documented browser wire, pre-auth counter, gauge, audit, and redaction coverage. |
| WP-C — token loop/framer/binding/registries/e2e | PARTIAL | CI plus full-pipeline registry-removal and wire-level coverage. |

AC1–12: **all PARTIAL**, pending CI regression and the coverage gaps listed above; none is claimed DONE. AC8 source handoff is addressed, but remains PARTIAL pending CI regression and automatic-close ordering validation.

### Final user response

PARTIAL: approved cancellation-registration ownership and the token-only cleanup-before-close barrier are implemented in source, including heartbeat-close coordination and early browser permit release. Formatting, layout, and whitespace checks pass. CI compilation and runtime validation, plus the documented acceptance-coverage gaps, remain outstanding. A child violated the no-compilation rule by launching cargo test; the identified process tree was terminated and no test pass is claimed. No commits, pushes, manifest changes, moves, or file deletions were performed.
