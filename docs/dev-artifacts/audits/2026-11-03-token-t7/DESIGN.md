# TOKEN-T7 Step 1 — authoritative coverage audit

## Evidence and status semantics

This is the single authoritative matrix for design §15 integration requirements. Each channel × case is a separate row (68 rows: 60 base channel/case cells plus 8 expanded kick/roles/password subcases). Source was inspected; **DONE means an assertion/test implementation exists in the cited source, not that it compiled or passed**. No compiler, test, CI, or runtime result was observed. `OBSERVED` is source-level evidence only. Paths are repository-relative. `PARTIAL` marks some but not all named subcases evidenced; `BLOCKED` means the requested direct proof cannot currently be authored without the noted seam or prohibited production change. `GAP` means no adequate assertion located. Browser WS details follow the channel matrix.

Design §15 matrix cases are interpreted per transport: HTTP body vs WS frame; slow request head remains applicable to HTTP and WS upgrade handshakes. HTTPS/WSS parity is not inferred from plaintext. HTTP missing Authorization is not covered by the older authentication test, but is now asserted by `http_and_https_channel_missing_header_and_query_token_are_unauthorized`.

## Channel matrix — 68 expanded rows (60 base cells plus 8 kick/roles/password subcases)

| Channel | Case | Source evidence (exact test) | Status |
|---|---|---|---|
| HTTP | valid read | `crates/shamir-server/tests/e2e/http_rest/lifecycle.rs::http_rest_allowed_owner_read_and_shutdown_restart_preserve_data` — observed read | DONE |
| HTTP | valid write | `crates/shamir-server/tests/e2e/http_rest/rails.rs:153-195::http_rest_remote_plaintext_opt_in_allows_executor_after_write_rail` — successful insert/read-back | DONE |
| HTTP | read-only token writing | `crates/shamir-server/tests/e2e/http_rest/policy.rs::http_rest_scope_denial_and_read_only_write_return_forbidden` | DONE |
| HTTP | database outside allowlist | same policy test | DONE |
| HTTP | wrong secret | `crates/shamir-server/tests/e2e/http_rest/authentication.rs::http_rest_unknown_bad_mac_revoked_expired_are_uniform_unauthorized` | DONE |
| HTTP | unknown id | same authentication test | DONE |
| HTTP | expired | same authentication test | DONE |
| HTTP | revoked after connect / next request | `crates/shamir-server/tests/e2e/token_matrix/suite/lifecycle.rs::admin_revoke_invalidates_online_token` — partial HTTP body is completed after revoke; final request returns 401 | DONE |
| HTTP | owner kicked | TOKEN_API §5 explicitly leaves `kick_session` unaffected; REST token invalidation is distinct | BLOCKED SPEC |
| HTTP | owner roles changed | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::owner_role_change_invalidates_rest_tokens` (tls=false and tls=true) — 401 after `grant_role` | DONE |
| HTTP | owner password changed | No matching REST invalidation test located | GAP |
| HTTP | oversized body | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::http_channel_header_flood_slow_head_and_oversize_are_bounded` lines 9-75 — oversized request body bounded | DONE |
| HTTP | header flood | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::http_channel_header_flood_slow_head_and_oversize_are_bounded` lines 9-75 — header flood bounded | DONE |
| HTTP | slow/incomplete head | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::http_channel_header_flood_slow_head_and_oversize_are_bounded` lines 9-75 — incomplete head timeout/EOF | DONE |
| HTTP | missing Authorization header | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::http_and_https_channel_missing_header_and_query_token_are_unauthorized` lines 423-472 — HTTP 401, uniform body/headers | DONE |
| HTTP | token in query ignored | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::http_and_https_channel_missing_header_and_query_token_are_unauthorized` lines 423-472 — query-only HTTP request 401 | DONE |
| HTTP | duplicate Authorization | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::http_channel_duplicate_authorization_is_rejected` lines 474-489 — 401 | DONE |
| WS | valid read | `crates/shamir-server/tests/e2e/http_ws/native.rs::http_ws_native_group` → `execute_rw_and_session_features`; read query assertion | DONE |
| WS | valid write | same exact test/group; successful write/transaction assertions | DONE |
| WS | read-only token writing | same test/group and `http_ws/session_loop.rs::exercise_native_session`; denial asserted | DONE |
| WS | database outside allowlist | `crates/shamir-server/tests/e2e/http_ws/native.rs::execute_rw_and_session_features` lines 502-525; asserts `token_database_denied` | DONE |
| WS | wrong secret | `crates/shamir-server/tests/e2e/http_ws/native.rs::native_error_matrix` via `http_ws_native_group` | DONE |
| WS | unknown id | same native error matrix | DONE |
| WS | expired | same native error matrix | DONE |
| WS | revoked after connect (4011) | `crates/shamir-server/tests/e2e/http_ws/revocation.rs::http_ws_revocation_expiry_group` | DONE |
| WS | owner kicked | TOKEN_API §5 explicitly leaves `kick_session` unaffected; WS token invalidation is distinct | BLOCKED SPEC |
| WS | owner roles changed | `crates/shamir-server/tests/e2e/http_ws/revocation.rs::http_ws_revocation_expiry_group` lines 525-565; role invalidation / close 4011 | DONE |
| WS | owner password changed | No matching assertion located | GAP |
| WS | oversized frame | `crates/shamir-server/tests/e2e/http_ws/native.rs::frame_close_cases` via `http_ws_native_group`, lines 561-595; oversized frame closes 1009/reset | DONE |
| WS | header flood | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::ws_channel_header_flood_and_slow_upgrade_head_are_bounded` lines 523-550 — 431 | DONE |
| WS | slow/incomplete upgrade head | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::ws_channel_header_flood_and_slow_upgrade_head_are_bounded` lines 523-550 — 408 and EOF | DONE |
| WS | missing Authorization header | `crates/shamir-server/tests/e2e/http_ws/native.rs::native_error_matrix` via `http_ws_native_group` | DONE |
| WS | token in query ignored | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::ws_channel_query_token_is_ignored` lines 588-633 — plain WS query-only attempt rejected 401 | DONE |
| WS | duplicate Authorization | `crates/shamir-server/tests/e2e/http_ws/native.rs::native_error_matrix` via `http_ws_native_group` | DONE |
| HTTPS | valid read | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::https_channel_data_and_scope_matrix` (lines 179-242) — token read after write/read-back | DONE |
| HTTPS | valid write | same test lines 210-242 — successful insert, followed by read-back | DONE |
| HTTPS | read-only token writing | same test lines 243-260 — 403 `token_read_only` | DONE |
| HTTPS | database outside allowlist | same test lines 261-284 — 403 `token_database_denied` | DONE |
| HTTPS | wrong secret | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::https_channel_wrong_secret_and_unknown_id_are_unauthorized` lines 491-521 — wrong secret and unknown ID 401 | DONE |
| HTTPS | unknown id | same authentication test, lines 491-521 | DONE |
| HTTPS | expired | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::tls_auth_gap` (one test per endpoint and case: `tls_{rest,wss}_auth_gap_*`) — expired credential 401 | DONE |
| HTTPS | revoked after connect / next request | same helper and per-case tests — revoked token next REST request 401 | DONE |
| HTTPS | owner kicked | TOKEN_API §5 explicitly leaves `kick_session` unaffected; REST token invalidation is distinct | BLOCKED SPEC |
| HTTPS | owner roles changed | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::owner_role_change_invalidates_rest_tokens` (tls=false and tls=true) — 401 after `grant_role` | DONE |
| HTTPS | owner password changed | No matching HTTPS invalidation test located | GAP |
| HTTPS | oversized body | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::https_channel_header_flood_slow_head_and_oversize_are_bounded` lines 64-76; shared framing assertions lines 41-55; 413 | DONE |
| HTTPS | header flood | same test; shared framing assertion lines 16-18; 431 | DONE |
| HTTPS | slow/incomplete head | same test; shared framing assertion lines 26-39; 408 and EOF | DONE |
| HTTPS | missing Authorization header | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::tls_auth_gap` (one test per endpoint and case: `tls_{rest,wss}_auth_gap_*`) — 401 with uniform body and headers | DONE |
| HTTPS | query token ignored | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::http_and_https_channel_missing_header_and_query_token_are_unauthorized` lines 423-472 — raw GET query-token request returns 401 | DONE |
| HTTPS | duplicate Authorization | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::tls_auth_gap` (one test per endpoint and case: `tls_{rest,wss}_auth_gap_*`) | DONE |
| WSS | valid read | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::wss_channel_data_scope_and_auth_matrix` lines 635-675 invokes `ws_data`; lines 72-122 writes and then reads actual `matrix-ws` record | DONE |
| WSS | valid write | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::wss_channel_data_scope_and_auth_matrix` lines 635-675 invokes `ws_data`; lines 72-122 successful data write | DONE |
| WSS | read-only token writing | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::wss_channel_data_scope_and_auth_matrix` invokes `ws_data`; lines 123-144 asserts token_read_only | DONE |
| WSS | database outside allowlist | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::wss_channel_data_scope_and_auth_matrix` invokes `ws_data`; lines 145-177 asserts token_database_denied | DONE |
| WSS | wrong secret | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::tls_auth_gap` (one test per endpoint and case: `tls_{rest,wss}_auth_gap_*`) — 401 | DONE |
| WSS | unknown id | same helper and per-case tests — 401 | DONE |
| WSS | expired | same helper and per-case tests — 401 | DONE |
| WSS | revoked after connect (4011) | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::wss_oversized_frame_and_revoke_closure` lines 136-195 — live close 4011 and session count reaches zero | DONE |
| WSS | owner kicked | TOKEN_API §5 explicitly leaves `kick_session` unaffected; WSS token invalidation is distinct | BLOCKED SPEC |
| WSS | owner roles changed | No WSS token invalidation assertion located | GAP |
| WSS | owner password changed | No matching WSS invalidation test located | GAP |
| WSS | oversized frame | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::wss_oversized_frame_and_revoke_closure` lines 79-134 — oversized frame close 1009 | DONE |
| WSS | header flood | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::wss_channel_header_flood_and_slow_upgrade_head_are_bounded` lines 552-585 — 431 | DONE |
| WSS | slow/incomplete upgrade head | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::wss_channel_header_flood_and_slow_upgrade_head_are_bounded` lines 552-585 — 408 and EOF | DONE |
| WSS | missing Authorization header | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::tls_auth_gap` (one test per endpoint and case: `tls_{rest,wss}_auth_gap_*`) — 401 | DONE |
| WSS | token in query ignored | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::ws_channel_query_token_is_ignored` loops over WS/WSS (`false`/`true`) query-only attempts rejected 401 | DONE |
| WSS | duplicate Authorization | `crates/shamir-server/tests/e2e/token_matrix/suite/framing.rs::tls_auth_gap` (one test per endpoint and case: `tls_{rest,wss}_auth_gap_*`) — 401 | DONE |

## Remaining design §15 integration requirements — individual subcases

| Requirement / subcase | Source evidence | Status |
|---|---|---|
| Cross-listener replay: token sid over SCRAM rejected | `crates/shamir-server/tests/e2e/http_ws/session_loop.rs::exercise_native_session` calls `assert_token_sid_rejected_over_scram`; asserts session_expired | DONE |
| Cross-listener replay: SCRAM sid over token session rejected (4010) | `crates/shamir-server/tests/e2e/http_ws/native.rs::execute_rw_and_session_features` submits SCRAM sid and asserts close 4010 | DONE |
| Token sid absent from SCRAM SessionStore | No direct store-absence assertion located | GAP |
| Privilege: superuser-owned token, every admin DbRequest | Classifier private; no exported exhaustive fixture/list, no socket sweep | BLOCKED |
| Privilege: replicator-owned token, every admin DbRequest | Same missing seam | BLOCKED |
| Privilege: superuser-owned token, every admin BatchOp | Same missing seam | BLOCKED |
| Privilege: replicator-owned token, every admin BatchOp | Same missing seam | BLOCKED |
| Privilege: Repl | No per-owner real-socket assertion; classifier list is private | BLOCKED |
| Privilege: ChangePassword* | No per-owner real-socket assertion; classifier list is private | BLOCKED |
| Privilege: List::Databases | Normative scope denies only when the requested database is outside a scoped allowlist; blanket denial is incorrect. No exhaustive token socket case located | PARTIAL |
| Privilege: List::Users | Normative scope denies only when requested database is outside a scoped allowlist; blanket denial is incorrect. No exhaustive token socket case located | PARTIAL |
| Privilege: AccessTree(db) | Normative scope permits when db is allowlisted and denies out-of-scope db; blanket denial is incorrect. No exhaustive token socket case located | PARTIAL |
| Subscription stops push after revoke without client action | `crates/shamir-server/tests/e2e/http_ws/revocation.rs::http_ws_revocation_expiry_group` lines 408-418; revoke then close 4011 and zero sessions without client request | DONE |
| Revoke race: in-flight requests, none authorized after revoke returns | No deterministic race test; WS authorization synchronization seam absent | BLOCKED |
| 16 active token sockets do not block owner SCRAM login | `crates/shamir-server/tests/e2e/http_ws/shutdown.rs::http_ws_shutdown_group` lines 20-80; opens/counts 16 and completes SCRAM login/ping | DONE |
| Pre-auth body flood remains under global body budget | `crates/shamir-server/tests/e2e/token_matrix/suite/lifecycle.rs::preauth_declared_body_flood_does_not_consume_authenticated_body_capacity` lines 37-83 tests 16 unauthenticated partial declared-body requests get 401 without 100 Continue; lines 85-167 then admits 16 authenticated reservations, rejects the 17th with 503, verifies unauthenticated request remains 401 while full, releases reservations and observes authenticated Pong. **PARTIAL:** shows this scenario/capacity outcome, not peak memory or a budget counter/full global-budget guarantee. | PARTIAL |
| Response-byte guard held until body written | No real-socket assertion located; ServerHandle exposes no response-budget counter/assertion seam | BLOCKED |
| Issuance: user creates own token | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::ordinary_user_issues_lists_and_revokes_own_tokens` lines 39-97 — own token create asserted | DONE |
| Issuance: user lists own tokens | same test — own listing asserted; secret absent from serialized listing | DONE |
| Issuance: user revokes own token | same test — own revoke and subsequent 401 asserted | DONE |
| Issuance: foreign user list denied | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::ordinary_user_cannot_delegate_or_manage_foreign_tokens` — foreign list denied | DONE |
| Issuance: foreign user revoke denied | same test — foreign revoke denied | DONE |
| Issuance: nonadmin `for_user` denied | same test — nonadmin delegation denied | DONE |
| Issuance: admin signed `for_user` | same test — admin `for_user` creation evidenced | DONE |
| Issuance: admin unsigned-HMAC control | No unsigned-HMAC control fixture/assertion located; raw fixture could be shared pending extraction. NOT IMPLEMENTED; no external impossibility claimed | GAP |
| Issuance: TTL cap | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::token_issuance_caps_are_enforced` — invalid TTL rejection and max TTL asserted | DONE |
| Issuance: maximum user token cap | same test — per-user cap asserted | DONE |
| Issuance: `allow_self_service=false` | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::disabled_self_service_retains_list_and_revoke` — ordinary create denied; listing/revoke available | DONE |
| Issuance: token-session create denied | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::token_sessions_cannot_manage_tokens_over_rest` — create denied | DONE |
| Issuance: token-session list denied | same test — list denied | DONE |
| Issuance: token-session revoke-user denied | same test — revoke-user denied | DONE |
| Issuance: token-session revoke-token denied | same test — revoke-token denied | DONE |
| Issuance: secret absent from listing | `crates/shamir-server/tests/e2e/token_matrix/suite/issuance.rs::ordinary_user_issues_lists_and_revokes_own_tokens` — serialized list lacks secret segment and full token | DONE |
| Issuance: secret exactly once after decode | same test lines 67-82 — decoded `created` value contains full token once; no wire-level exactly-once proof | PARTIAL |
| Issuance: password change revokes tokens | No matching assertion located | GAP |
| Channels: Rest-only refused at WS upgrade (403/no 101) | `crates/shamir-server/tests/e2e/http_ws/native.rs::exercise_native_group` lines 680-698; asserts 403 and token_channel_denied | DONE |
| Channels: Ws-only refused on REST | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::https_channel_data_and_scope_matrix` lines 318-330 — HTTPS 403 `token_channel_denied` | DONE |
| Channels: Both works on REST and WS using same token | Same test lines 332-365 — one Both token succeeds on HTTPS REST Pong and WSS Pong | DONE |
| Log redaction: real socket REST and native WS success/failure; bearer secret and MAC absent | `crates/shamir-server/tests/e2e/token_matrix/suite/observability.rs::token_listener_socket_traces_redact_rest_and_native_ws_credentials` — captured tracing around successful/failed REST and WS requests; checks complete bearer, secret encodings and MAC encodings absent | DONE |
| Online revoke: partial-body request rejected after revocation; prior committed data readback intact | `crates/shamir-server/tests/e2e/token_matrix/suite/lifecycle.rs::admin_revoke_invalidates_online_token` — HTTP 100-continue, partial body, revoke, finish body, final 401 and read control observes only positive seed | DONE |
| Restore invalidation floor, old-token denial and fresh-token acceptance | `crates/shamir-server/tests/e2e/token_matrix/suite/lifecycle.rs::restore_invalidates_tokens_and_allows_fresh_issuance` — backup/restore, floor assertion, existing token 401 and fresh token 200 | DONE |
| WS subscription push through token session | `crates/shamir-server/tests/e2e/http_ws/revocation.rs::http_ws_revocation_expiry_group` observes push before revoke | DONE |
| WS cursors through token session | `crates/shamir-server/tests/e2e/http_ws/native.rs::execute_rw_and_session_features` creates/fetches/cancels cursor | DONE |
| REST `not_supported_over_rest` for transaction | `crates/shamir-server/tests/e2e/token_matrix/suite/channel_matrix.rs::https_channel_data_and_scope_matrix` lines 371-398 — HTTPS request returns 400 and `not_supported_over_rest`; HTTP/1 parity not covered or required for this generic integration bullet | DONE |
| REST `not_supported_over_rest` for cursor | Same test lines 371-398 — HTTPS cursor request returns 400 `not_supported_over_rest`; HTTP/1 parity not covered or required | DONE |
| REST `not_supported_over_rest` for subscription | Same test lines 371-398 — HTTPS subscription request returns 400 `not_supported_over_rest`; HTTP/1 parity not covered or required | DONE |
| Windows CI | No CI result observed | BLOCKED |
| Linux CI | No CI result observed | BLOCKED |
| No sleeps across new/relevant lifecycle tests | Source requirement; no suite-wide audit/result | PARTIAL |

## Browser WebSocket source audit

`crates/shamir-server/tests/e2e/http_ws/browser.rs::http_ws_browser_group` is existing source evidence, not a runtime result. Lines 23-49 exercise successful browser handshake with Host/Origin; lines 51-105 authenticate a first token frame, assert no pre-credential binding, binding fields, and call refusal, first-frame, canonical-credential, cap, and pre-auth-capacity subgroups. Lines 137-201 demonstrate eight pre-auth sockets consume capacity, receive 503/Retry-After, then invalid first-frame auth closes 4000; capacity is retested after release. Lines 203-225 demonstrate active per-token cap close 1013. Lines 227-294 cover canonical unknown, wrong-MAC, revoked, expired credentials as 4000. Lines 296-339 cover malformed/unmasked/wrong-size first frames, invalid token, Rest-only token close 4013, trailing/malformed payload, and timeout (no frame). Lines 382-461 cover Host/Origin absent, invalid, duplicate, path, and allowlist cases (400/403). Source covers several browser §10 concerns; it does not establish browser log redaction, exhaustive global resource accounting, all §10 requirements, compilation, or green behavior.

## Substantive blockers and limitations

- Privilege enumeration is not implemented: `crates/shamir-server/src/security/perimeter/http_rest/scope.rs::classify_request` (lines 97-134) and `classify_batch` (149-251) are private exhaustive classifiers; no exported exhaustive fixture/list exists. Also, blanket metadata denial conflicts with normative scope: `ListDatabases`/`ListUsers` deny only for out-of-scope databases; `AccessTree(db)` is permitted for an allowlisted db.
- Direct SCRAM-store absence and body/response budget counters are not asserted through current `ServerHandle`; no public SCRAM store, body-budget counter, or response-budget counter. Do not alter production to expose them. An internal raw SCRAM fixture exists in `http_ws/session_loop.rs`, but is not currently a public fixture for this assertion; pending fixture extraction, NOT IMPLEMENTED (not an external impossibility). Auth-before-body has an implemented lifecycle child using HTTP `Expect: 100-continue` without inventing counters.
- WS revoke-race lacks an authorization synchronization seam; no deterministic assertion is evidenced.
- Other matrix rows remain GAP where no assertion was found. Password-change invalidation and unsigned-HMAC control assertions remain NOT IMPLEMENTED; do not claim they cannot be done. WSS owner-role/password invalidation remain missing; REST owner-role invalidation now has HTTP/HTTPS source evidence. Owner kick semantics are explicitly unaffected by TOKEN_API §5; the separate REST-token/WS-token invalidation expectation remains a specification conflict and is classified BLOCKED SPEC, not unspecified. No assertion was weakened. HTTPS query-token ignored is DONE above. HTTP/1 query and WS/WSS query coverage are distinct.
- Dependency pins are not changed by this task (`Cargo.lock` is tracked and untouched). Compilation, tests, and CI are unverified.


## Run progress (historical append-only log)

- 13:56 CET — Step 1 started; brief, CLAUDE.md, design §15 and named TOKEN_API/TRANSPORT_WS specs read before prior audit.
- Audit source inventory completed; direct test names recorded and unit/component tests explicitly not promoted to real-socket coverage.
- 13:56 CET — Step 1 started; brief, CLAUDE.md, design §15 and named TOKEN_API/TRANSPORT_WS specs read before prior audit.
- Corrective read-through — fully read SECURITY_MODEL.md and relevant named REST/WS test bodies: HTTP write/read-back, WS database-scope, oversized frame, owner role invalidation, both replay directions, revoke without client request, 16-socket SCRAM accounting and Rest-only upgrade refusal. Corrected earlier false GAPs.
- Exhaustiveness check — `scope.rs` private exhaustive matches confirmed; no exported fixture/list seam found. Privilege sweep is blocked on that missing seam; no production changes.
- DONE — source-only audit; issuance, lifecycle and observability progress recorded after reading actual test sources. No commands/tests run. Exhaustive privilege list, metadata-scope cases, direct SCRAM-store/budget assertions, WS authorization race, password-change and unsigned-HMAC controls remain unimplemented/unverified. Browser §10 review and CI evidence remain unmet.
- 13:56 CET — Step 1 started; brief and CLAUDE.md policy read. Compiler/build/test execution explicitly prohibited by task override; no commands run.
- Progress update — all Rust files reread by orchestrator; formatting has run and layout passed earlier; final checks are soon. Current fixture seeds nested from the start (not moved). Overall remaining matrix gaps include REST/WSS/password owner invalidation, exhaustive private privilege cases, direct SCRAM-store/response-budget assertions, WS revoke race, browser §10 and CI. Unsigned-HMAC admin control remains separately NOT IMPLEMENTED. No build or tests run here.
- Finish-status update — overall status remains PARTIAL. The issuance matrix was split into individual subcases. Source review includes all seven token-matrix Rust files and `crates/shamir-server/tests/e2e/main.rs`; this is source review, not compile/test evidence. Formatting check was observed successful (`cargo fmt -p shamir-server -- --check`, exit 0); layout OK, zero baseline, zero hints. No Rust compile, tests, or CI ran. REST owner-role invalidation is source-reviewed in `issuance.rs:264-325` for HTTP and HTTPS; the log-redaction capture is bounded at 8 MiB and the fixture is nested from the start. WSS owner-role and password invalidation remain missing; HTTP and HTTPS query-token gap is now DONE. Explicit TOKEN_API §5 unaffected kick semantics conflict with token-invalidation expectations, classified BLOCKED SPEC. Remaining gaps/blockers include exhaustive privilege sweep/private classifier seam, normative metadata-scope contradiction, direct SessionStore/response-budget proof, WS authorization race, peak full-body-budget proof, wire-level exactly-once proof, password-change and unsigned-HMAC controls, and other browser §10/CI evidence. Source review found no sleeps, TODOs or allows; token_matrix has 7 direct entries; each source file stays within 1000 lines. No full-task completion is claimed.
- Finished checks — `cargo fmt -p shamir-server -- --check` exit 0; layout OK, zero baseline, zero hints.

## Final report message

PARTIAL — added the token coverage audit and real-socket matrix, issuance, revocation-race, budget-admission, restore, and redaction tests under `crates/shamir-server/tests/e2e/token_matrix/`; registered them in `crates/shamir-server/tests/e2e/main.rs`.
Checks: `cargo fmt -p shamir-server -- --check` and `python -I scripts/ci/check_layout.py` passed. No compilation or tests ran.
Remaining: exhaustive privilege sweep and specification conflicts; WSS owner-role/password invalidation, password-change and unsigned-HMAC issuance controls, direct session-store/response-budget proof, deterministic WS authorization race, and wire-level secret-once proof. Full details and unverified claims: `docs/dev-artifacts/audits/2026-11-03-token-t7/DESIGN.md`.
No production code, dependencies, manifests, lockfiles, or workflows changed; no commits or git mutations were made.
OBSERVED: an empty untracked root file `framing` appeared in the worktree during the run (origin unverified); the orchestrator removed it before committing.
