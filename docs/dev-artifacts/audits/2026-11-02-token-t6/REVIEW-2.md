# TOKEN-T6 review 2 - revision 1 (`ffd1e2f27`) against REVIEW-1

Scope: worktree `worktrees/p34-t6`, branch `fix/p34-t6-ws-token`, HEAD `ffd1e2f27` (revision 1) on top of `b4e1b0470`
(cooldown exception), `0c06cd45d` (WIP first slice + step 0) and the brief commit `2c9b08b41`; base master `f11889057`.

Method: read-only. `git diff f11889057 HEAD`, every changed/new file read end to end, dependency sources in the cargo
registry (hyper 1.9.0, scc 3.8.4, rmp-serde 1.3.1, tungstenite 0.29.0 as the nearest available proxy for the locked
0.30.0; tokio-tungstenite 0.30 sources are not in the registry). Tools run: `rustfmt --check --edition 2021` on every
changed `.rs` file (no drift), `python -I scripts/ci/check_layout.py` (`layout check: OK (0 baselined, 0 hint(s))`),
`ls -A` on every touched folder. No cargo, no build, no tests. Line numbers are as of `ffd1e2f27`.

Severity as in REVIEW-1: **S1** compile error; **S2** fails a non-rustc CI gate (`clippy -D warnings`, rust-guard
`modules`, layout, fmt) or breaks an existing test; **S3** behaviour contradicts brief/spec, or a test oracle is wrong,
vacuous or flaky; **S4** hygiene.

## 0. Verdict

Revision 1 closed every REVIEW-1 S1 item and every S2 item; by reading, the tree now type-checks except for the items
below that I can only rate "likely". It introduced one definite regression in an existing transport test (N1), two
probable `clippy::type_complexity` failures (N2), one definite `clippy::useless_vec` (N3), a Windows-only close/RST
race in three e2e cases (N4), and two vacuous e2e oracles in the revocation group (N5). The ownership handoff
(registration moved into teardown, close-after-cleanup barrier) is sound. Nothing is DONE until CI is green; with
N1-N3 fixed the fast tier has a realistic chance of passing.

---

## 1. REVIEW-1 items re-verified

### 1.1 S1 (compile errors) - 26 items

| # | Status | Evidence (current file:line) |
|---|---|---|
| A1 | FIXED | `shamir-transport-http/src/server/service.rs:1` `use super::body::response_body;` |
| A2 | FIXED | `server/routes/ws/handler.rs:14` `mut request` |
| A3 | FIXED | `server/tests/ws_route_tests.rs:1` `use super::super::{serve, serve_with_ws};` |
| W1 | FIXED | `shamir-transport-ws/src/tests/framing_tests.rs`: diff vs master is additions only, existing test untouched |
| B1 | FIXED | `http_ws/mod.rs:8` `mod upgrade;` |
| B2 | FIXED | `boot/config/http_listener.rs:6` `use crate::http_ws::browser::policy::canonical_authority;` (visible: `http_ws` pub, `browser`/`policy` `pub(crate)`) |
| B3 | FIXED | `http_ws/route.rs:142-145`, `:282-322` (`activate(&self, Verified, &mut WsSession)`, no field moves out of the `Drop` type) |
| B4 | FIXED | `route.rs:238-240` `Arc::new(Verified { state: Arc::clone(&grant.state) })` |
| B5 | FIXED | `server_launcher/http/launcher.rs:14-15` |
| B6 | FIXED | `launcher.rs:101` `Arc::clone(&username_lookup)` |
| B7 | FIXED | `http_rest/tests/scope_tests.rs:1` |
| B8 | FIXED | `scope_tests.rs:499`, `:585` carry `query_version: 1` |
| B9 | FIXED | `http_ws/tests/policy_tests.rs:149-200` call `validate_head` with 5 args |
| C1 | FIXED | `sync/durability/tx_registry.rs:153` single `owner_sid`; master `owner_user_id` untouched |
| C2 | FIXED | `tx_registry.rs:311-314` single `remove_for_session` (guard dropped at end of the `let`) |
| C3 | FIXED | `token_ws/token_loop.rs:129` `let (writer, reader) = stream.split();` |
| C4 | FIXED | no `Arc::new(ctx.grant)`; `ctx.registration` is the only field moved out of `TokenLoopContext` (no `Drop`), `:171` |
| C5 | FIXED | `token_loop.rs:20`, `:24` (`crate::byte_budget`, `crate::http_rest::{HandleDenial, ScopedHandler}`) |
| C6 | FIXED | `token_ws/binding.rs` has no `new_binding`; re-export `token_ws/mod.rs:11` matches |
| C7 | FIXED | `token_ws/tests/binding_tests.rs:5-6` (`super::super` = `token_ws`) |
| C8 | FIXED | `token_loop.rs:51-55` `pub(super) struct ErrorReply` + fields, `:65` `pub(super) fn encode_prereserved` |
| C9 | FIXED | `binding_tests.rs:21-26`, `:55-60` use `BTreeMap<String, IgnoredAny>`; no `rmpv` anywhere |
| E1 | FIXED | `tests/e2e/http_rest/common.rs:17`, `:241` `pub(crate)` |

### 1.2 S2 (lint / gate / existing tests) - 29 items

| # | Status | Evidence |
|---|---|---|
| A4 | FIXED | `service.rs:5` imports only `WsRoute, WsUpgrade` (both used) |
| A5 | FIXED | `server/accept.rs:22` `type BoxedIo = Box<dyn IoStream>;` (private) |
| A6 | FIXED | no `GuardedIo::into_inner` in `framing/guarded_io.rs` |
| A7 | FIXED | `ws_route_tests.rs:29`, `:43` `async fn` |
| B10 | FIXED | `http_ws/mod.rs:6-7` `#[cfg(test)] mod tests;` |
| B11 | FIXED | `http_ws/` = 6 entries, `http_ws/browser/` = 4, `http_ws/tests/` = 5 |
| B12 | FIXED | `http_rest/mod.rs:14-19` (`pub use gate::{RestGate, RestGateRails}` + doc restored, `pub(crate) use gate::BrowserVerdict`) |
| B13 | FIXED | `http_rest/scope.rs:59-72` |
| B14 | FIXED | `gate_tests.rs:10` no unused alias |
| B15 | FIXED | `gate_tests.rs:155` `#[tokio::test]` |
| B16 | FIXED | `redaction_tests.rs:13` |
| B17 | FIXED | `maintenance.rs:484-495` keeps `summary_total`, `failure_dropped`, `exposure_dropped`, master message |
| B18 | FIXED | `route.rs:149` `async fn run` |
| B19 | FIXED | both `now_ns()` in `activate` (`:283`, `:306`) are used (deadline, recheck) |
| B20 | FIXED (superseded) | no `map_or(true, ..)` |
| B21 | FIXED | `upgrade.rs:1-9` |
| B22 | FIXED | `upgrade.rs:12-14` `ValidatedHead { accept }` only |
| B23 | FIXED | `browser/first_frame.rs:28` |
| B24 | FIXED | `cancel_registry.rs:103` `let _removed = occupied.remove_entry();` |
| B25 | FIXED | `policy_tests.rs:1-6` |
| B26 | FIXED | `rustfmt --check` clean on every changed file |
| C10 | FIXED | `request_loop.rs:183` + `:219` (`&mut rx` on the captured outer binding) |
| C11 | FIXED | `token_ws/framer.rs:4` `SinkExt` (used by `send`/`close`) |
| C12 | FIXED | `framer.rs` has only `with_heartbeat` constructors |
| C13 | FIXED | no `drop(envelope)` |
| C14 | FIXED | `token_loop.rs:368-405` no trailing `return;` |
| C15 | FIXED | `binding_tests.rs:1-6` |
| E2 | FIXED | `revocation.rs:2` `SinkExt` now used (`:97`); `shutdown.rs:3` `StreamExt` only |

### 1.3 S3 (behaviour) - 33 items

| # | Status | Evidence / residual |
|---|---|---|
| 1 REST shutdown path | FIXED | `accept.rs:237-285`: cancel branch returns `timeout(..).ok()`, upgrade hand-off first, then master drain/shutdown/linger under `into_parts()` |
| 2 handler placement | FIXED | `routes/ws/handler.rs`; `service.rs:44-51` only branches |
| 3 tunnel oracles | FIXED | `framing/tests/tunnel_tests.rs:55-76` (`expect_err` on 200 ms read), `:78-124` paused idle-tunnel test |
| 4 route tests | FIXED | `ws_route_tests.rs:229-366` (NoWs both paths, 405 before hook, 503 + `Retry-After`, extension header stripped `:210-212`, shutdown cancels `run`) |
| 5 `mirror.rs` | FIXED | no `shamir-connect` diff vs master |
| 6 REST exposure tuple | FIXED | `maintenance.rs:640-650` `(channel_label(channel), id, owner)`; REST test unchanged; WS twins `maintenance_tests.rs:678-730` |
| 7 unrelated churn | PARTIAL | restored: "Reserve before inserting" `:104`, "two-second shutdown limit" `:464`, REST log strings. Still churned vs master: `gate.rs:55` and `:63` docs trimmed (`shutdown` cancellation note, `# Panics` body), `maintenance.rs:59` doc reworded, `:118` `inserted` -> `value`, removed blank lines (`:70`, `:101`, `:166`, `:466`), `select!` arms `:509`, `:522` collapsed, `should_flush` inlined `:597`, `:659` warn reflowed to one 110-char line |
| 8 scope tests | FIXED | REST tests differ from master only by `, Endpoint::Rest` (plus `impl IntoBatchOp` in two helpers, cosmetic); `expected_ws` exhaustive without `_` `scope_tests.rs:445-467` |
| 9 maintenance tests | FIXED | only signature updates in existing tests |
| 10 Host normalisation | FIXED | `browser/policy.rs:39-93` = REVIEW-1 code |
| 11 per-request re-validation | FIXED | removed from `route.rs` and `upgrade.rs`; deploy text documents exact matching |
| 12 limiter feeding | FIXED | `route.rs:161` feeds on frame defects; `Failed` arm `:175` does not double-feed |
| 13 activation codes | FIXED | `route.rs:257-278` `ActivationError` (1013/4011/4012), all closes use `close_reason` `:338` |
| 14 trailing bytes / `bin` | FIXED | `first_frame.rs:92-125` byte-exact parser |
| 15 pre-auth race | FIXED | `browser/pre_auth.rs:36-47` reserve under the entry guard; `in_use` test accessor `:55-63` |
| 16 global pre-auth | FIXED | one `Arc<PreAuthAdmission>` in `launch.rs:580`, passed via `ListenerEnv` (`listeners.rs:75`) |
| 17 Origin/Host order | FIXED | `upgrade.rs:78-95` |
| 18 local `use` | FIXED | `upgrade.rs:3`, `route.rs:17-19` hoisted |
| 19 duplicate-Origin test | FIXED | `policy_tests.rs:64-67` |
| 20 missing unit tests | FIXED (mostly) | `first_frame_tests.rs`, `pre_auth_tests.rs`, `policy_tests.rs`, gate/redaction WS tests; still no route-level permit-counter test |
| 21 teardown before close | FIXED | `token_loop.rs:411-421` cleanup -> `cleanup_complete` -> writer finish; `close_guard.rs:44-48`, `:57-61` |
| 22 `application_error` | FIXED | `token_loop.rs:399-404` |
| 23 WS metrics | FIXED (code) | `token_loop.rs:366-367`, `:381-382`; launcher pre-registers `ws` labels `launcher.rs:155-166` |
| 24 stash outside scope | FIXED | `token_loop.rs:359-365` uses `run_with_handoff`'s guard |
| 25 constants | FIXED | `token_loop.rs:33`, `:237`; `launcher.rs:184` `CONN_MAX_IN_FLIGHT` |
| 26 bare writer spawn | NOT FIXED (accepted) | `token_loop.rs:177` bare `tokio::spawn`, aborted by `WriterTaskGuard`; recorded as deviation in DESIGN |
| 27 `is_ok_and` | FIXED | `token_loop.rs:296-298` |
| 28 own-module imports | FIXED | `token_loop.rs:19`, `:30` |
| 29 writer comments | FIXED | `session/writer_task.rs:52-66` |
| 30 binding expiry oracle | FIXED | `e2e/http_ws/session_loop.rs:340` `== created.record_view.expires_at_ns` |
| 31 identical 401 matrix | FIXED | `e2e/http_ws/native.rs:75-169` (missing/duplicate/malformed/unknown/wrong/revoked/expired, Date stripped, XFF-isolated subnets) |
| 32 caps / limiter | FIXED | `native.rs:171-230` (per-token and per-owner `503` + `Retry-After: 1`), `:714-739` (`429`, then valid token upgrades) |
| 33 revocation coverage | PARTIAL | live subscription + revoke `revocation.rs:302-419`, `revoke_user` `:499-525`, role change `:526-566`, ttl `:483-497`; the tx-rollback and cursor oracles are vacuous (N5); registry entry absence not asserted end to end |

### 1.4 S4 and 1.5 leftovers

| Item | Status | Evidence |
|---|---|---|
| `token_ws/tests/` directory | FIXED | `token_ws/tests/{mod.rs,binding_tests.rs,framer_tests.rs,teardown_tests.rs}` |
| `//!` in `http_ws/tests/mod.rs` | FIXED | manifest is 4 `mod` lines (note: `tests/e2e/http_ws/mod.rs:1` still has a `//!` line, harmless) |
| widenings listed | FIXED | DESIGN.md "Observed visibility widenings"; `proxy.rs` unchanged vs master |
| deploy wildcard note | FIXED | `deploy/server.example.ktav:87-93` |
| DESIGN.md stale / duplicated reports | NOT FIXED | five "Final report"/"Final user response" sections, contradictory history (e.g. `:152` "401" corrected at `:111` and `:161`), header still "work item 1". Replace with one current report |
| 1.5 leftovers (tx_registry, binding, framer ctors, token_loop stale lines, route re-validation, policy_tests, scope placeholder, tunnel test, guarded_io helper, accept churn) | FIXED | all gone |

---

## 2. New defects introduced or still open after revision 1

### N1 - S2 (definite): existing transport test breaks - WS paths bypass the gate when `endpoints.ws = false`

`crates/shamir-transport-http/src/tests/transport_tests.rs:813` (master test, unchanged, `setup` uses `serve` with
`EndpointSet { rest: true, ws: false }` at `:165`) expects `GET /shamir/v1/ws` -> `404` **with exactly one gate call**
(`:837-842`). Now `service.rs:44-51` sends every WS path to `handle_ws`, which answers `404` at `handler.rs:34-40`
without calling the gate -> `gate.calls == 0`, assertion fails. Violates brief decision 2 ("transport tests must not
churn") and acceptance 12. Fix: take the WS branch only when the WS endpoint is enabled and restore master's REST-path
text for the disabled case (byte-identical master behaviour):

```rust
// service.rs, replacing :44-59 and the method match at :120-124
    let ws_route = match request.uri().path() {
        "/shamir/v1/ws" if context.cfg.endpoints.ws => Some(WsRoute::Native),
        "/shamir/v1/ws/browser" if context.cfg.endpoints.ws => Some(WsRoute::Browser),
        _ => None,
    };
    if let Some(route) = ws_route {
        return super::routes::ws::handler::handle_ws(request, context, route).await;
    }
    let started = tokio::time::Instant::now();
    let deadline = started + context.cfg.limits.request_timeout;
    let path = request.uri().path().to_owned();
    let endpoint = if path == "/shamir/v1/ws" {
        Endpoint::Ws
    } else {
        Endpoint::Rest
    };
    let known = match path.as_str() {
        "/shamir/v1/request" | "/shamir/v1/ping" => context.cfg.endpoints.rest,
        "/shamir/v1/ws" => context.cfg.endpoints.ws,
        _ => false,
    };
    // ... unchanged ...
    let method = match path.as_str() {
        "/shamir/v1/request" => Method::POST,
        "/shamir/v1/ping" | "/shamir/v1/ws" => Method::GET,
        _ => Method::GET,
    };
```

and delete `handler.rs:34-40` (now unreachable). `ws_route_tests` use `ws: true` and keep passing.

### N2 - S2 (probable): `clippy::type_complexity` on two new signatures

Clippy's visitor adds `10 * nest` per path/tuple and also walks the qualified self of `Self::X` / `W::X`; the default
threshold is 250 (no override in `clippy.toml`). Trait declarations are checked (`check_trait_item`), trait impls are
not.

* `shamir-transport-http/src/server/routes/ws/hook.rs:118` (trait `WsUpgrade::admit` return):
  `Result<(Self::Session, Vec<(http::HeaderName, http::HeaderValue)>), WsReject>` scores 290.
* `shamir-transport-http/src/server/service.rs:23-25` (field `Context::upgrade`):
  `tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<(W::Session, hyper::upgrade::OnUpgrade)>>>` scores 260.

Fix (aliases are not scored):

```rust
// hook.rs
/// Admitted session plus the extra `101` response headers chosen by the application.
pub type WsAdmission<S> = (S, Vec<(http::HeaderName, http::HeaderValue)>);
//  in the trait:
    fn admit(
        &self,
        route: WsRoute,
        head: HeadOk<Self::HeadState>,
        grant: Option<G>,
        peer: PeerInfo,
    ) -> Result<WsAdmission<Self::Session>, WsReject>;
// routes/ws/mod.rs and lib.rs: add `WsAdmission` to the `pub use hook::{..}` lists.
// Impls (`NoWs`, `WsUpgradeService`, test `Hook`) may keep the expanded tuple: the types are identical.

// service.rs
/// Per-connection hand-off slot for the upgraded session and hyper's pending upgrade.
type UpgradeSlot<S> =
    tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<(S, hyper::upgrade::OnUpgrade)>>>;
//  in Context:
    pub upgrade: UpgradeSlot<W::Session>,
```

### N3 - S2 (definite): `clippy::useless_vec` in a test target

`crates/shamir-server/src/security/perimeter/http_ws/tests/first_frame_tests.rs:88` `&vec![0; 126]` (126 bytes, under
the 200-byte `too-large-for-stack` default; this exact class was auto-fixed workspace-wide in `6c0d6bd69`). Fix:
`assert!(read_one(frame(9, true, 0, &[0_u8; 126])).await.is_err());`. (`browser.rs:246` `&vec![0; 257]` and
`framing_tests.rs:51` `&vec![0; 16 MiB + 1]` are above the threshold and do not fire.)

### N4 - S3 (Windows flake, likely deterministic there): close frame lost to RST when the server drops a socket with unread input

The REST path lingers (`accept.rs:279-283` `discard_inbound`) precisely so that a peer is not reset before reading the
final response; the WS terminal paths do not. When the server closes the TCP socket with unread bytes in its receive
buffer the OS sends RST; on Windows a received RST discards data the client has not read yet, so the client sees
`ConnectionReset` instead of the close frame. Affected e2e cases (Windows integration cell of the full tier):

* `e2e/http_ws/browser.rs:245` text first frame (server reads the 2-byte header, 5 bytes stay unread) and `:246`
  257-byte first frame (4 header bytes read, 261 unread) -> `assert_close_code` `expect("close message")` panics;
* `e2e/http_ws/native.rs:575-585` full 16 MiB + 1 payload (the client is still writing megabytes when the server closes
  1009).

Fix in the route (covers every `4000/4013/4014/1013/4011` close of `route.rs`):

```rust
// route.rs header
use tokio::io::AsyncReadExt;
/// Unread client bytes drained after a terminal close so the socket ends with FIN, not RST.
const CLOSE_LINGER_LIMIT: usize = 64 * 1024;
const CLOSE_LINGER: Duration = Duration::from_secs(2);

async fn close(socket: &mut WebSocketStream<UpgradedIo>, code: u16, shutdown: &CancellationToken) {
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            _ = shutdown.cancelled() => {},
            _ = socket.send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: crate::framer::close_reason(code).into(),
            }))) => {},
        }
    })
    .await;
    let io = socket.get_mut();
    let mut scratch = [0_u8; 1024];
    let mut drained = 0_usize;
    let drain = async {
        while drained < CLOSE_LINGER_LIMIT {
            match io.read(&mut scratch).await {
                Ok(0) | Err(_) => break,
                Ok(read) => drained += read,
            }
        }
    };
    tokio::select! {
        _ = shutdown.cancelled() => {}
        _ = tokio::time::timeout(CLOSE_LINGER, drain) => {}
    }
}
```

The 16 MiB case cannot be made reliable by a bounded drain; keep the header-only case (`native.rs:561-573`) as the
strict 1009 oracle and accept a reset for the full payload:

```rust
    let outcome = tokio::time::timeout(Duration::from_secs(15), oversized_payload_socket.next())
        .await
        .expect("close deadline");
    match outcome {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 1009),
        Some(Err(tokio_tungstenite::tungstenite::Error::Io(error)))
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ) => {}
        other => panic!("expected 1009 or reset after an oversized payload: {other:?}"),
    }
```

### N5 - S3 (definite): two revocation oracles cannot fail

* `e2e/http_ws/revocation.rs:432-451` and `:464-480`: "staged write must roll back" reads `items` from another session.
  An uncommitted interactive-transaction write is never visible to another session, with or without teardown, so the
  assertion passes even if `TokenSessionTeardown` never rolls back.
* `revocation.rs:452-463`: `FetchNext` of the old cursor from a **different** session returns `cursor_not_found` even if
  the cursor still exists: `engine/db_handler/cursor_handlers/fetch.rs:636` maps `CursorOwnershipMismatch` to
  `cursor_not_found`.

Fix: keep them as smoke checks (rename the messages to "not visible to other sessions") and add oracles that fail when
the feature is absent, at the layer that owns them:

```rust
// src/tests/registry/tx_registry_tests.rs (fixture `make_tx` exists at :16)
#[tokio::test]
async fn remove_for_session_removes_only_that_sessions_tx() {
    let registry = TxRegistry::new(/* same args as the other tests in this file */);
    let sid = [5_u8; 32];
    let (handle, tx) = make_tx(sid, Duration::from_secs(60), 1).await;
    registry.register(handle, tx).expect("register");          // use the file's register call shape
    let removed = registry.remove_for_session(&sid).expect("owned tx removed");
    assert_eq!(removed.owner_sid(), &sid);
    assert!(registry.remove_for_session(&sid).is_none());
    assert!(registry.get_owned(handle, &sid).is_err());         // or the file's lookup helper
}
// src/tests/registry/cursor_registry_tests.rs: open two cursors for sid A and one for sid B, call
// remove_all_for_session(&A): returns 2 cursors, A's ids are gone, B's cursor still resolves.
```

(The exact constructor/lookup names must be copied from the two existing test files; the shape above is the oracle.)

### N6 - S3 (low): shutdown before activation ends without `1000`

`route.rs:157` (browser first-frame wait) and `:224` (binding send) `return` on `shutdown.cancelled()` without a close
frame; TOKEN_API 10 says shutdown closes `1000`. Fix: `_ = shutdown.cancelled() => { drop(session.permit.take()); let
mut socket = websocket(io).await; close(&mut socket, 1000, &CancellationToken::new()).await; return; }` for `:157`, and
`close(&mut socket, 1000, &CancellationToken::new()).await; return;` for `:224` (use a fresh token so `close` does not
skip the send; it stays bounded by its own 2 s timeout).

### N7 - S3 (low): native gate timeout answers `401`, REST answers `408`

`handler.rs:60` maps `Err(Elapsed)` to the uniform `401`. TOKEN_API 5 keeps "действующие HTTP-статусы" for the native
route. Fix: split the arm: `Err(_) => return Ok(canned(StatusCode::REQUEST_TIMEOUT, context.peer.tls, context.cfg.hsts)),`.

### N8 - S3 (low): WS exposure drops are reported in the REST summary

`maintenance.rs:394`, `:409` count every exposure drop in the single `exposure_drops`, and `:559`, `:575` add it to
`summaries[0]` (REST): a dropped WS exposure makes a REST `api_token_auth_failures` record. Fix: add
`ws_exposure_drops: Arc<AtomicU64>` (5th `DropCounters` field), pick the counter by `channel` in `record_exposed`, and
add it to `summaries[1].exposure_dropped`. Optional for this task.

### N9 - S4: existing config test renamed

`boot/config/tests/http_listener_tests.rs:412` renamed `http_listener_rejects_zero_head_timeout_and_malformed_proxy_cidrs`
to `..._and_authorities` and gained a case. A rename reads as "lost test" to `rust-guard discovery` whenever a campaign is
active. Fix: restore the master name and drop the added `example.com:80` case (already covered by
`canonical_host_authorities_validate_by_listener_scheme`, `:544-583`).

### N10 - S4: hygiene

* REVIEW-1 S3-7 residual churn listed in 1.3 (restore master text of `gate.rs:55-66`, `maintenance.rs:59`, `:70`,
  `:101`, `:116-123`, `:166`, `:466`, `:509`, `:522`, `:597-603`, `:659`).
* Stale docs: `http_ws/upgrade.rs:1` ("authentication, admission, and session activation" - it only validates heads),
  `browser/policy.rs:3` ("Returns true" - returns `Option<String>`), `token_ws/teardown.rs:39` ("after the route
  increments the gauge" - this call increments it).
* DESIGN.md: one current report (see 1.4).

### Checked and clean (no finding)

`split()` order; `Arc<Verified>` handoff; no field moved out of `WsSession`/`PreAuthPermit`/`TokenWsRegistration`/
`TokenSessionTeardown`; `Send` of every spawned or `run` future (all trait futures are declared `+ Send`; nothing
non-`Send` is held across an `.await`; tokio `select!` drops branch futures before handlers, so `io` is movable in the
`read_browser_token` arm); `result_large_err` (largest error `WsReject`, 24 bytes); `manual_async_fn` (none);
`private_interfaces` (`RestHandler::new`, `ScopedHandler::handle`, `BrowserVerdict`, `ListenerEnv` fields consistent);
`collapsible_if` (edition 2021: nested `if let` is not linted; plain nested `if` were fixed); `drop_non_drop` (every
`drop(..)` argument needs drop); `must_use` (scc `remove_entry` bound; `insert_entry`, DashMap `remove_if`,
`TokenSessionStore::remove` are not `must_use`); `absolute_paths` (no non-`use` path above 5 segments outside
`std`/`tokio`); `too_many_arguments` (max 7: `serve_inner`, `serve_with_ws`); rust-guard: every `mod.rs` re-exports
only, no inline `#[cfg(test)]` bodies, the only local `use`s are pre-existing (`tx_registry.rs:238`, `framer.rs:421`),
no new `#[allow]`, every new file is mounted (`modules` gate) and every test manifest is `#[cfg(test)]`-gated from its
parent; layout: all touched folders <= 7 entries (`server/` 7, `token_ws/` 7, `sync/connection/` 7, `session/` 7,
`e2e/http_ws/` 7), largest touched file 828 lines (`launch.rs`); `scc::len()` not used; Fx hashers everywhere.

---

## 3. Ownership handoff and lifecycle (design check)

* Registration: created in `activate` (`route.rs:303-305`) after the store admission, then rechecked
  (`:310`) - register-then-recheck holds against publish-then-cancel (`TokenService::revoke` publishes, cancels, then
  removes store sessions). The registration moves `WsSession -> TokenLoopContext -> TokenSessionTeardown`
  (`route.rs:248-251`, `token_loop.rs:166-172`); every early route exit drops it before writing the close
  (`route.rs:162-163`, `:177`, `:184`, `:191`, `:199`, `:226`). Sound.
* Close barrier: the writer's `close_with`/`shutdown` go through `TokenWsCloseGuard` (`close_guard.rs:44-61`), which
  signals `close_requested` and waits for `cleanup_complete`; the loop's three `select!`s all listen on
  `close_requested` first (`token_loop.rs:213`, `:242`, `:280`), and cleanup runs synchronously before
  `cleanup_complete.cancel()` (`:411-416`). Heartbeat timeout (writer-initiated 4003), write errors and loop-initiated
  codes (4010/4011/4012/1000/1003/1009) all observe store removal, tx removal, cursor removal, registry unregister and
  gauge decrement before the close frame. No deadlock path found: the only waits on the barrier are inside the writer
  task under its 2 s `timeout`, and `WriterTaskGuard::finish` is bounded by 5 s plus abort.
* Residual risk (not a defect): if cleanup takes longer than the writer's 2 s close timeout the close frame is skipped
  (client sees 1006). Cleanup is synchronous and O(cursors), so this needs a pathological registry.

---

## 4. Acceptance status (brief items 1-12)

Nothing has compiled; "code" = implementation present by reading, "oracle" = a test that fails if the feature is absent.

| # | Status | Present | Missing / blocking |
|---|---|---|---|
| 1 | PARTIAL (code + oracle complete) | `route.rs`, `upgrade.rs`, `binding.rs`; e2e `session_loop.rs:32-80`, `:340` | CI |
| 2 | PARTIAL (nearly complete) | identical-401 matrix, 400/403/405, Origin with empty list, per-token and per-owner 503, 429 then valid upgrade (`native.rs`) | e2e Origin vs non-empty non-matching list (unit only, `policy_tests.rs:108-122`); "missing header does not feed the limiter" not asserted; N7 |
| 3 | PARTIAL | gate rails + `api_token_exposed` with `("ws", id, owner)` at unit level (`gate_tests.rs:182-229`) | no wire test on a remote-plaintext listener |
| 4 | PARTIAL | Origin/Host matrix e2e (`browser.rs:315-394`) + unit, "no frame before credentials" (`browser.rs:79-83`), pre-auth caps unit | `404` on plaintext non-loopback untested anywhere; pre-auth `503` not on the wire; permits-return-to-zero only for `PreAuthAdmission` in isolation |
| 5 | PARTIAL | first-frame unit table; e2e text/257/invalid/trailing/bin-key/timeout -> 4000 with fixed reason, unknown/wrong/revoked/expired -> 4000, rest-only -> 4013, cap -> 1013 | 4014 only at gate level; route-level permit release unasserted; N4 (Windows) |
| 6 | PARTIAL (nearly complete) | read/write Execute, Tx*, cursors, admin and database denials, read-only denials inside `res`, exact reply keys (`session_loop.rs:115-120`), invalid_envelope / invalid_request shapes, Subscribe + push keys (`revocation.rs`) | `rate_limited` envelope untested |
| 7 | PARTIAL (oracle complete) | other socket's sid, zero sid, SCRAM sid -> 4010; token sid over SCRAM -> `session_expired` | CI |
| 8 | PARTIAL | revoke with live subscription -> 4011 without push, `revoke_user`, role change, ttl -> 4012, store counts 0 | N5 (tx/cursor oracles vacuous), registry-entry absence not asserted end to end |
| 9 | PARTIAL | 1003, 1009 (header-only and full payload), heartbeat 4003 unit, shutdown 1000 + `report.errors().is_empty()` | N4 for the full-payload case |
| 10 | PARTIAL (oracle complete) | `shutdown.rs`: 16 token sockets, SCRAM login + ping succeed | CI |
| 11 | PARTIAL | metrics emitted in code; `channel = ws` summaries/exposures unit-tested; gate-level redaction for both routes | no metric value asserted (`auth_total{channel="ws"}`, `active_ws` up/down, `request_denied_total{endpoint="ws"}`, `duration{endpoint="ws"}`); no route-level log capture |
| 12 | FAILING until N1 | SCRAM writer extraction identical; REST/scope/maintenance tests unchanged except signatures | N1 breaks `routing_and_gate_statuses_have_expected_headers_and_bodies`; N9 rename |

---

## 5. Finishing list (about 2 hours)

Order matters: 1-3 unblock the fast tier.

1. **N1** (15 min) - `service.rs` guarded WS branch + master text; delete `handler.rs:34-40`.
2. **N2** (15 min) - `WsAdmission` and `UpgradeSlot` aliases; add `WsAdmission` to both re-export lists.
3. **N3** (2 min) - `first_frame_tests.rs:88` array literal.
4. Run the allowed local gate on the four crates (`cargo fmt -p .. -- --check`, `cargo clippy -p shamir-transport-http
   -p shamir-transport-ws -p shamir-server --all-targets -- -D warnings`, layout) if the user allows `cargo clippy`;
   otherwise push a draft and read the fast tier. This settles N2 and everything in section 6 that depends on lint
   internals.
5. **N4** (25 min) - route `close` linger; relax the full-payload assertion in `native.rs:575-585`.
6. **N5** (30 min) - registry unit tests for `TxRegistry::remove_for_session` and
   `CursorRegistry::remove_all_for_session` copying the existing fixtures; reword the two e2e messages.
7. **N9 + N10 + S3-7 residual** (20 min) - restore the master test name and the master text of the listed lines;
   fix the three stale docs.
8. **N6, N7** (10 min) - shutdown 1000 on pre-activation exits; 408 on gate timeout.
9. **Acceptance 4/3 wire gaps** (20 min, optional within the window) - in an existing remote-plaintext REST e2e
   launch (`tests/e2e/http_rest/rails.rs`), add: browser route `404`, native `read_write` upgrade `403
   token_plaintext_write_denied`, `read_only` upgrade `101`. No new launch.
10. DESIGN.md: replace the five stacked reports with one current final report (10 min).

Deferred (outside 2 h): N8; metric-value assertions (needs a process-global recorder in the e2e binary); wire-level
pre-auth `503`; `rate_limited` envelope test.

---

## 6. What this review could not verify

* Lint internals: the `type_complexity` scores in N2 assume clippy walks the qualified self of `Self::X`/`W::X`
  (it does in `walk_qpath` as far as I know); `useless_vec` on `let v = vec![..]; f(&v)` bindings (I concluded it does
  not fire because the coercion sits on `&v`, not on `v`).
* tokio-tungstenite / tungstenite 0.30.0 exact APIs (`WebSocketConfig` builder setters, `Utf8Bytes: From<&str>` and
  `PartialEq<&str>`, `CloseCode: From<u16>`): checked against 0.29.0 only.
* Runtime: hyper 1.9 upgrade through `GuardedIo` (whether the `101` head is always observed as one
  `"HTTP/1.1 101 Switching Protocols"` status line before the dispatcher hands over), tungstenite's ability to send a
  close after a `Capacity` error, scheduling of the paused-clock tests, Windows RST semantics (N4 is inferred from
  documented Winsock behaviour, not observed).
* e2e builder/client signatures beyond the ones checked (`Batch::{insert, query, create_table, create_db, subscribe,
  grant_role, id}`, `Subscribe::with_initial`, `ConnectOptions` fields, `HandshakeBuilder` flow): most are copies of
  existing tests (`mvp_e2e.rs`, `http_rest/lifecycle.rs`) and were spot-checked, not exhaustively.
* That `grant_role` through `UserAdminPort` raises `tickets_invalid_before_ns` (the revocation role-change case relies
  on it; `user_directory/directory.rs` sets the floor in the role-update paths, the exact `grant_role` branch was not
  traced).
* The JS client end to end.
