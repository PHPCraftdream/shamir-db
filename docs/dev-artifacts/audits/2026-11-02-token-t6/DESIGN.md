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
Acceptance 10: PARTIAL — admission permit remains held through WS run; SCRAM accounting proof is later work. Coverage: `ws_route_returns_hook_headers_and_holds_permit_through_session`; CI needed.
Acceptance 11: PARTIAL — metrics/audit/redaction are later work.
| Acceptance 12: PARTIAL — no changes made to SCRAM framing/loop behavior. CI must run existing transport tests and the new `tunnel_passes_arbitrary_binary_bytes_both_directions_after_101`, `bytes_before_101_remain_inside_the_http_scanner`, `serve_uses_no_ws_and_returns_not_found`, and `ws_route_returns_hook_headers_and_holds_permit_through_session`. The pre-101 test only checks the current parked-read behavior; it does not prove a rejection/scanner postcondition. |

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
Acceptance 11: PARTIAL — audit/metrics candidate exists; REST assertion was changed contrary to acceptance 12, and redaction/channel aggregation are unverified.
Acceptance 12: PARTIAL — REST assertion change violates the acceptance constraint; SCRAM parity and framing behavior are not verified.

No compile or test suite was run in this audit-only pass. Parent-reported HTTP/WS formatting success, server formatting failure, and layout-check success are not compilation or behavioral evidence. Known source blockers must be fixed before CI; all acceptance items remain PARTIAL pending successful compilation and relevant tests.

