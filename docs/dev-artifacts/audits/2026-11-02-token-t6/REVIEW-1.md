# TOKEN-T6 review 1 - state of the uncommitted executor work

Scope: worktree `worktrees/p34-t6`, branch `fix/p34-t6-ws-token`, on top of master `f11889057` + brief commit `2c9b08b41`.
Inputs: `git status --short`, `git diff`, every untracked file, the brief
`docs/dev-artifacts/prompts/remediation/p34/t6-ws-token-channel.md`, `DESIGN.md` in this folder, TOKEN_API.md section 1/5/7/8/10/13/15,
dependency sources in the cargo registry (hyper 1.9.0, tungstenite / tokio-tungstenite 0.30.0, rmp-serde 1.3.1, scc 3.8.4).

Method: read-only. No cargo, no build, no tests. The only tool run besides reading was `rustfmt --check --edition 2021` on single
files (read-only, no target dir). Everything below is what a compiler/clippy/rust-guard/layout run is expected to report, derived by
reading the code. Line numbers are as of this review.

Severity: **S1** compile error (blocks every CI tier); **S2** fails a CI gate other than rustc (`clippy -D warnings`, rust-guard
`modules`, layout, fmt) or breaks an existing test; **S3** behaviour contradicts the brief/spec, or a new test is wrong; **S4** brief
rule / discipline / hygiene (does not fail CI today).

Note on rust-guard in CI: `.rust-guard/stages.toml` has `campaigns = []`, so the `structure` gate (MODRS_LOGIC, LOCAL_USE,
INLINE_TEST_MOD, VIS_WIDENING) is skipped in CI today; only `modules` (reachability) and `orderings` run. The structure rules are
still brief rules and are listed as S4. The layout checker ignores untracked files, so the `http_ws/` 8-entry violation will only show
up after commit, as S2.

---

## 1. Defect list (severity-ordered)

### 1.1 S1 - compile errors

#### shamir-transport-http

| # | Where | Defect | Fix |
|---|---|---|---|
| A1 | `src/server/service.rs:1` (use at `:469`) | `use super::body::response_body;` was deleted, `response_body(...)` still called at `:469` (E0425). | Re-add `use super::body::response_body;` at the top. |
| A2 | `src/server/service.rs:28` (use at `:141-143`) | `request.extensions_mut()` on a non-`mut` parameter (E0596). | `mut request: Request<Incoming>,` in `handle_ws`. |
| A3 | `src/server/tests/ws_route_tests.rs:1` | `use super::accept::{serve, serve_with_ws};` - the tests module is mounted inside `accept` (`accept.rs:353-355`), so `super` is `accept::tests`, there is no `accept::tests::accept` (E0432). | `use super::super::{serve, serve_with_ws};` (same pattern as `accept_tests.rs:1`). |

#### shamir-transport-ws

| # | Where | Defect | Fix |
|---|---|---|---|
| W1 | `src/tests/framing_tests.rs:201-202` | Duplicate `#[tokio::test]` on the EXISTING test `rv_transport_ws_b8ed1366_heartbeat_pong_does_not_consume_control_budget` (tokio-macros: "second test attribute is supplied"). Also edits an existing SCRAM test (acceptance 12). | Delete line 202. |

#### shamir-server, security / boot / launcher

| # | Where | Defect | Fix |
|---|---|---|---|
| B1 | `src/security/perimeter/http_ws/mod.rs` | `upgrade.rs` is not declared: `route.rs:23` `use super::upgrade::...` and `tests/policy_tests.rs:5` fail (E0432); `upgrade.rs` is also unreachable for rust-guard `modules` (S2). | Add `mod upgrade;` (see the restructured `mod.rs` in WP-B step 0). |
| B2 | `src/boot/config/http_listener.rs:6` | `crate::security::perimeter::...` - `perimeter` is a private module of `security` (`security/mod.rs:7 mod perimeter;`), not visible from `boot` (E0603). Also `browser_policy` is a private module of `http_ws` (`http_ws/mod.rs:3`). | `use crate::http_ws::browser::policy::canonical_authority;` after the WP-B restructure, with `pub(crate) mod browser;` / `pub(crate) mod policy;`. |
| B3 | `src/security/perimeter/http_ws/route.rs:155-159` and `:204-208` | Moving fields out of `activated: WsSession`, a type that implements `Drop` (`:66`) (E0509). | Replace `activate(...) -> Result<WsSession, WsReject>` with an activation that fills `&mut WsSession` (code in WP-B item B-r3). |
| B4 | `src/security/perimeter/http_ws/route.rs:238-240` | `TokenLoopContext.grant` is `Arc<Verified>` (`token_loop.rs:34`), a bare `Verified` is passed (E0308). | `grant: Arc::new(Verified { state: Arc::clone(&grant.state) }),` (contract with WP-C: the field type stays `Arc<Verified>`). |
| B5 | `src/engine/server/server_launcher/http/launcher.rs:13-14` | `use crate::http_rest::{RestGate, RestGateRails, RestHandler};` was replaced by the `http_ws` import, but `RestGate` (`:23`), `RestGateRails` (`:83`), `RestHandler` (`:95`) are still used (E0412/E0433). | Add back `use crate::http_rest::{RestGate, RestGateRails, RestHandler};` next to `use crate::http_ws::{PreAuthAdmission, WsUpgradeService};`. |
| B6 | `src/engine/server/server_launcher/http/launcher.rs:99` and `:163` | `username_lookup` is moved into `RestHandler::new` at `:99`, then `Arc::clone(&username_lookup)` at `:163` (E0382). | `:99` -> `Arc::clone(&username_lookup),`. |
| B7 | `src/security/perimeter/http_rest/tests/scope_tests.rs:1-14` | `use crate::security::perimeter::http_rest::AllowedOperation;` was deleted, `AllowedOperation::` is still used 7 times (E0433). | Re-add `use crate::security::perimeter::http_rest::AllowedOperation;` (inside the crate's own `http_rest` subtree this path is legal). |
| B8 | `src/security/perimeter/http_rest/tests/scope_tests.rs:88` and `:185` | `DbRequest::TxCommit { db, tx_handle, visibility_wait }` without the required `query_version` field (E0063). | Add `query_version: 1,` (as at `:550`). |
| B9 | `src/security/perimeter/http_ws/tests/policy_tests.rs:117-128` and `:135-146` | `validate_head` is called with 6 arguments, it takes 5 (`upgrade.rs:17-23`) (E0061). Leftover from an earlier signature. | Drop the second `false` argument: `validate_head(route, head, false, &policy, &[...])`. |

#### shamir-server, sync / engine

| # | Where | Defect | Fix |
|---|---|---|---|
| C1 | `src/sync/durability/tx_registry.rs:152-165` | Three `pub fn owner_sid` definitions (E0592); the original `owner_user_id() -> &[u8; 16]` accessor was overwritten, while `src/tests/registry/tx_registry_tests.rs:71` calls `got.owner_user_id()` (E0599), and the `owner_user_id` field becomes unread (`dead_code`, S2). | Keep the first `owner_sid` (`:150-155`), replace `:157-165` with exactly the master text: `/// Owning user id.` `pub fn owner_user_id(&self) -> &[u8; 16] { &self.owner_user_id }`. |
| C2 | `src/sync/durability/tx_registry.rs:311-326` | `remove_for_session` defined twice (E0592). | Delete `:319-326` (the second copy). The kept one uses `owner_sid`, which is the right key (DESIGN.md errata is correct on this). |
| C3 | `src/sync/connection/token_ws/token_loop.rs:117` | `let (reader, writer) = stream.split();` - `StreamExt::split` returns `(SplitSink, SplitStream)`, so `reader` is the sink (E0308 at `:120-121`). | `let (writer, reader) = stream.split();` |
| C4 | `src/sync/connection/token_ws/token_loop.rs:131` | `let grant = Arc::new(ctx.grant);` moves `ctx.grant`; later `&ctx.grant` (`:196`, `:224`) and `Arc::clone(&ctx.grant)` (`:271`) use the moved field (E0382); `grant` itself is never used (`unused_variables`). | Delete line 131. `still_valid(&ctx.grant, now)` then auto-derefs `&Arc<Verified>` to `&Verified`. |
| C5 | `src/sync/connection/token_ws/token_loop.rs:25-26` | `crate::security::perimeter::http_rest::...` and `crate::security::perimeter::byte_budget` - `perimeter` is private to `security` (E0603). | `use crate::byte_budget;` and `use crate::http_rest::{HandleDenial, ScopedHandler};` (both are re-exported at the crate root, `lib.rs:33-36`). |
| C6 | `src/sync/connection/token_ws/binding.rs:33` | `grant.state.record().expires_at_ns()` - `expires_at_ns` is a field of `TokenRecord`, not a method (E0599). | Delete `new_binding` (`:23-39`) entirely: it is unused (route.rs builds the session itself), and its re-export in `token_ws/mod.rs:10` is then an `unused_imports` error. Remove the `Session`/`Verified` imports (`:18-19`) with it. |
| C7 | `src/sync/connection/token_ws/tests.rs:6-7` | `super::super::binding` / `super::super::token_loop` - for a file module `token_ws/tests.rs`, `super::super` is `connection` (E0432). | Resolved by the orchestrator move `tests.rs -> tests/binding_tests.rs` (step 0 below), after which `super::super` is `token_ws` and the paths are right. |
| C8 | `src/sync/connection/token_ws/tests.rs:7`, `token_loop.rs:45-49`, `:59` | `encode_prereserved` and `ErrorReply` (and its fields `rid`, `error`) are private to `token_loop` (E0603/E0451). | `pub(super) struct ErrorReply<'a> { pub(super) rid: Option<u32>, pub(super) error: &'a str }` and `pub(super) fn encode_prereserved`. |
| C9 | `src/sync/connection/token_ws/tests.rs:22`, `:59` | `rmpv` is not a dependency of `shamir-server` (normal or dev) (E0433). No `Cargo.toml` change is allowed. | Decode into `std::collections::BTreeMap<String, serde::de::IgnoredAny>` (BTreeMap is not banned) and assert `keys == ["expires_at_ns", "ok", "sid"]`; for the second test decode into `BTreeMap<String, serde::de::IgnoredAny>` and assert it has exactly `error` and `rid`. |

#### e2e (`crates/shamir-server/tests/e2e/`)

| # | Where | Defect | Fix |
|---|---|---|---|
| E1 | `http_ws/session_loop.rs:1` vs `http_rest/common.rs:17`, `:248` | `ADMIN_PASSWORD` and `grant_default_main_access` are `pub(super)` (visible in `http_rest` only) (E0603). | Make both `pub(crate)` in `http_rest/common.rs`. |

### 1.2 S2 - clippy `-D warnings`, rust-guard `modules`, layout, fmt, broken existing tests

| # | Where | Finding | Fix |
|---|---|---|---|
| A4 | `shamir-transport-http/src/server/service.rs:4` | `HeadOk` imported, unused (`unused_imports`). | `use crate::server::routes::ws::{UpgradeHead, WsReject, WsRoute, WsUpgrade};` |
| A5 | `shamir-transport-http/src/server/accept.rs:24` | `pub(crate) type BoxedIo = Box<dyn IoStream>;` exposes the module-private trait `IoStream` through a crate-visible alias (`private_interfaces`, warn-by-default); the widening is not needed (only used in `accept.rs`). | Revert to `type BoxedIo = Box<dyn IoStream>;`. |
| A6 | `shamir-transport-http/src/framing/guarded_io.rs:53-55` | New `GuardedIo::into_inner` is never called (`accept.rs:263` calls `TokioIo::into_inner`) -> `dead_code`. | Delete it. |
| A7 | `shamir-transport-http/src/server/tests/ws_route_tests.rs:29-36`, `:43-56` | `fn authenticate<'a>(..) -> impl Future + Send + 'a { async {..} }` and the same for `execute`: body is only an async block -> `manual_async_fn` (the class CI already hit in this program). | `async fn authenticate(&self, _: AuthHeader<'_>, _: crate::config::Endpoint, _: PeerInfo) -> GateDecision<()> { GateDecision::Allow(()) }` and `async fn execute(&self, _: &(), _: Bytes) -> ExecOutcome { ExecOutcome { .. } }` (pattern of `src/tests/lifecycle_tests.rs:28`). Drop the then-unused `std::future` paths. |
| B10 | `src/security/perimeter/http_ws/mod.rs:10` | `mod tests;` without `#[cfg(test)]`: in the lib target every `#[test]` fn is stripped, leaving the test imports unused and `good_headers()` dead -> `unused_imports`/`dead_code` errors in `clippy --all-targets`. | `#[cfg(test)] mod tests;` |
| B11 | `src/security/perimeter/http_ws/` | 8 direct entries (`browser_policy.rs cancel_registry.rs first_frame.rs mod.rs pre_auth.rs route.rs tests/ upgrade.rs`), limit 7. Fails `check_layout.py` once committed. | Orchestrator step 0: move `browser_policy.rs`, `pre_auth.rs`, `first_frame.rs` into `http_ws/browser/` (-> 6 entries). |
| B12 | `src/security/perimeter/http_rest/mod.rs:15-18` | `pub use gate::{RestGate, RestGateRails}` was narrowed to `pub(crate) use gate::{BrowserVerdict, RestGate}`, but the publicly reachable `RestHandler::new` (`executor.rs:91-97`) returns `super::RestGate` -> `private_interfaces` ("type `RestGate` is more private than the item `RestHandler::new`"). Also drops an existing doc comment. | `/// Gate and fixed listener security rails for T3b-3 wiring.` `pub use gate::{RestGate, RestGateRails};` plus a separate `pub(crate) use gate::BrowserVerdict;`. |
| B13 | `src/security/perimeter/http_rest/scope.rs:59-72` | `if matches!(..) { if endpoint == Endpoint::Rest { return .. } }` -> `collapsible_if`. | `if endpoint == Endpoint::Rest && matches!(request, DbRequest::TxBegin { .. } \| DbRequest::TxExecute { .. } \| DbRequest::TxCommit { .. } \| DbRequest::TxRollback { .. } \| DbRequest::CreateCursor { .. } \| DbRequest::FetchNext { .. } \| DbRequest::CancelCursor { .. }) { return Decision::NotSupported("not_supported_over_rest"); }` |
| B14 | `src/security/perimeter/http_rest/tests/gate_tests.rs:11` | `Endpoint as TokenEndpoint` imported, unused. | Remove it from the use list. |
| B15 | `src/security/perimeter/http_rest/tests/gate_tests.rs:157` | `async fn native_ws_auth_accepts_ws_token_and_rejects_rest_channel()` has no `#[tokio::test]`: never runs, `dead_code`. | Add `#[tokio::test]` above it. |
| B16 | `src/security/perimeter/http_rest/tests/redaction_tests.rs:13-15` | `Endpoint as TokenEndpoint` imported, unused. | `use shamir_connect::server::token::{OwnerFloors, TokenChannels, TokenMode};` |
| B17 | `src/security/perimeter/http_rest/maintenance.rs:470` | The shutdown-deadline warning lost `summary_total`, `failure_dropped`, `exposure_dropped`; the EXISTING test `stalled_writer_deadline_warns_before_detaching` (`tests/maintenance_tests.rs:480-482`) asserts `summary_total` and `failure_dropped` -> test failure (acceptance 12). | In that arm: `tracing::warn!(summary_total = summaries[0].total.saturating_add(summaries[1].total), pending_exposures = exposed.len(), failure_dropped = summaries[0].dropped.saturating_add(summaries[1].dropped), exposure_dropped = summaries[0].exposure_dropped, "REST token audit shutdown drain deadline expired");` (keep the original message, it is REST-owned). |
| B18 | `src/security/perimeter/http_ws/route.rs:164-170` | `fn run(..) -> impl Future<Output = ()> + Send { async move { .. } }` -> `manual_async_fn`. | `async fn run(&self, mut session: Self::Session, mut io: UpgradedIo, shutdown: CancellationToken) { .. }`; drop `use std::future::Future;` (`:3`). |
| B19 | `src/security/perimeter/http_ws/route.rs:261-262` | `let now = now_ns();` twice: first binding unused (`unused_variables`). | Delete `:261`. |
| B20 | `src/security/perimeter/http_ws/route.rs:298` | `.map_or(true, |grant| ..)` -> `unnecessary_map_or` (style, toolchain 1.95). | Superseded by B-r3; if kept: `.is_none_or(|grant| !self.service.still_valid(grant, now))`. |
| B21 | `src/security/perimeter/http_ws/upgrade.rs:3` | `use std::net::SocketAddr;` unused. | Delete. |
| B22 | `src/security/perimeter/http_ws/upgrade.rs:13` | `ValidatedHead.route` is written but never read -> `dead_code`. | Delete the field (route is carried in `WsHeadState.route`). |
| B23 | `src/security/perimeter/http_ws/first_frame.rs:30` | `opcode < 0x8 \|\| opcode > 0xA` -> `manual_range_contains`. | `!(0x8..=0xA).contains(&opcode)` |
| B24 | `src/security/perimeter/http_ws/cancel_registry.rs:222` | `occupied.remove_entry();` - scc 3.8.4 `OccupiedEntry::remove_entry` is `#[must_use]` (`hash_map.rs:1886`) -> `unused_must_use`. | `let _removed = occupied.remove_entry();` |
| B25 | `src/security/perimeter/http_ws/tests/policy_tests.rs:113` | `let peer: SocketAddr = ..` unused; `use std::net::SocketAddr;` (`:1`) becomes unused with it. | Delete both. |
| B26 | `route.rs`, `upgrade.rs`, `token_loop.rs` | `rustfmt --check` reports drift (6, 9 and 2 hunks). | `cargo fmt -p shamir-server` after the edits, then `-- --check`. |
| C10 | `src/sync/connection/request_loop.rs:216` | `let mut rx = rx;` inside the spawned block makes the outer `let (tx, mut rx)` (`:183`) an `unused_mut`. | Delete line 216; `&mut rx` inside the `async move` block then mutates the captured outer binding. |
| C11 | `src/sync/connection/token_ws/framer.rs:4` | `StreamExt` imported, unused (no `.next()` / `.split()` in this file). | `use futures_util::SinkExt;` |
| C12 | `src/sync/connection/token_ws/framer.rs:29-31`, `:44-46` | `TokenWsFrameReader::new` / `TokenWsFrameWriter::new` never called -> `dead_code`. | Delete both (only `with_heartbeat` is used). |
| C13 | `src/sync/connection/token_ws/token_loop.rs:269` | `drop(envelope);` on `RequestEnvelopeView` (borrowed slices, no `Drop`) -> `drop_non_drop` (suspicious). | Delete the line; NLL already ends the borrow. |
| C14 | `src/sync/connection/token_ws/token_loop.rs:361`, `:368` | Trailing `return;` in the last two arms of the tail `match` of the dispatch `async move` block -> `needless_return`. | Delete both `return;`. |
| C15 | `src/sync/connection/token_ws/tests.rs:4` | `use serde_bytes;` -> `single_component_path_imports`. | Delete (the `#[serde(with = "serde_bytes")]` attribute needs no import). |
| E2 | `tests/e2e/http_ws/revocation.rs:2`, `shutdown.rs:2` | `SinkExt` imported, unused (no `send`/`close`) in a test target compiled by `clippy --all-targets -D warnings`. | `use futures_util::StreamExt;` |

### 1.3 S3 - behaviour contradicting brief/spec, wrong or missing oracles

Transport (`shamir-transport-http`):

1. **REST shutdown path changed** - `src/server/accept.rs:239-259`. The cancel branch now returns `None` and `if connection_result.is_none() { return; }` skips the TCP write-shutdown that master performed for every finished connection (master kept `timeout(..).await.ok()` -> `Some(outcome)` and then always ran `AsyncWriteExt::shutdown`). REST behaviour must not change. Fix:
   ```rust
   let connection_result = tokio::select! {
       _ = token.cancelled() => {
           connection.as_mut().graceful_shutdown();
           tokio::time::timeout(cfg.limits.request_timeout, &mut connection).await.ok()
       }
       result = &mut connection => Some(result),
   };
   if let Ok((session, on_upgrade)) = upgrade_rx.try_recv() { /* unchanged: await upgrade, run, return */ }
   if let Some(outcome) = connection_result { /* unchanged drain / shutdown / linger, with `into_parts()` Option */ }
   ```
   Delete `:257-259`. Restore the deleted linger comment ("Linger: keep reading so a peer that still writes is not reset before it has read the final response.") and revert the churn at `:276-278` and `:290-297` to master's text (`if tokio::time::timeout_at(deadline, tasks.join_next()).await.is_err() { break; }`).
2. **WS route handler placement** - `src/server/service.rs:27-181`: the whole `handle_ws` + `ws_rejection` (155 lines) was put into `service.rs`; the brief requires only an early branch there and the handler under `src/server/routes/ws/` (AUTH1 edits `service.rs` in parallel; merge risk). Move both functions to a new `src/server/routes/ws/handler.rs` (`routes/ws/` has 2 entries), make `auth_header`, `retry`, `unauthorized_body` `pub(crate)` in `service.rs` (list the widenings in DESIGN.md). Behaviour unchanged.
3. `src/framing/tests/tunnel_tests.rs:52-78` `bytes_before_101_remain_inside_the_http_scanner` has no oracle (aborts the task, asserts nothing). Rewrite: client writes the head plus `[0x82, 0x85, ..]`; server reads the head; `tokio::time::timeout(Duration::from_millis(200), guarded.read(&mut one))` must return `Err(Elapsed)` (bytes not released before the `101`). The brief's "no deadline fires on an idle tunnel" test is missing: build with `.with_timeouts(1s, 1s, 1s)`, write the `101`, `tokio::time::advance(10s)` under `#[tokio::test(start_paused = true)]`, then a read and a write must still pass through.
4. `src/server/tests/ws_route_tests.rs` covers only "101 + hook headers + permit held" and "`serve` -> 404 on `/shamir/v1/ws`". Missing per the brief's test plan: `NoWs` -> 404 on `/shamir/v1/ws/browser`; non-GET -> 405 + `Allow: GET` without calling the hook; a hook rejection (`503`, `retry_after: Some(1)`) -> status + `Retry-After: 1`; `Sec-WebSocket-Extensions` from the hook is dropped; shutdown cancels the token passed to `run` (hook waits on it, server `shutdown.cancel()`, `done` fires).

shamir-connect:

5. `crates/shamir-connect/src/server/auth/token/mirror.rs:56` doc-comment edit only. The brief allows touching `shamir-connect` only when unavoidable. Revert (S4, listed here because it widens the touched-crate set for the fast tier).

Gate / scope / maintenance (`src/security/perimeter/http_rest/`):

6. **REST exposure audit details changed** - `maintenance.rs:580-602` writes `(id, owner)` for REST (master: `("rest", id, owner)`), and the existing assertion was edited to match (`tests/maintenance_tests.rs:704-706`). Violates "REST byte-identical" and acceptance 12; TOKEN_API section 13 also requires the channel in the record. Fix: merge the two `Exposed` arms into one:
   ```rust
   Some(AuditNotice::Exposed { channel, id, owner, subnet, now_ns }) => {
       match rmp_serde::to_vec_named(&(channel_label(channel), id, owner)) { /* existing Ok/Err bodies */ }
   }
   ```
   and restore the test to master: `let details: (String, [u8; 16], [u8; 16]) = ..; assert_eq!(details, ("rest".into(), id, owner));`. Add a WS twin of that test asserting `("ws", id, owner)`.
7. Unrelated comments deleted in `maintenance.rs` (master `:102-103` "Reserve before inserting; concurrent new keys cannot exceed the hard cap.", master `:437` "At the two-second shutdown limit, detach an in-flight blocking batch rather than delay shutdown.", master `:220` summary-tuple doc) and in `gate.rs:55-63` (`shutdown`/`new` panics doc). Restore. The `tokio::select!` arms in `run_flusher` (`:468-488`) were squashed into single 150-300 character lines (rustfmt does not touch `select!` bodies, so fmt passes); restore master's multi-line layout for every arm whose logic did not change. All other REST log messages changed from "REST token ..." to "token ..." - keep the master strings for REST (`channel_label(channel)` prefix only where the channel is known).
8. `tests/scope_tests.rs`: existing REST tests were restructured and renamed (`rest_request_allowlist_and_rest_only_transaction_class_are_distinct` -> `rest_allowlist_...`, inlined fixtures, deleted comment before `ApiTokenManagementWireFixture`). Acceptance 12 allows signature updates only. Restore the REST part from master and add only `, Endpoint::Rest` to each call. Keep the three new WS tests at the end of the file (WS classifier tests must stay here: `scope` is a private module of `http_rest`, not reachable from `http_ws/tests/`). Delete the leftover `let _ = Filter::And { filters: Vec::new() };` (`:207-209`) and its import (`:10`). Replace the index split in `ws_scope_classifies_all_db_requests_deny_by_default` (`:131-145`) by a `fn expected_ws(request: &DbRequest) -> Decision` with an exhaustive `match` over every `DbRequest` variant without `_`, so a new variant breaks compilation.
9. `tests/maintenance_tests.rs`: the added assertions (`:285`, `:297`, `:331-334`) and the rewritten lookup (`:627-643`) are edits of existing tests; revert them (keep only the `Endpoint::Rest` argument and `FailureSummary::new(Endpoint::Rest)` signature updates). Put new channel assertions into new tests.

`http_ws` (route / policy / first frame / pre-auth):

10. **Host/Origin are rejected instead of normalised** - `browser_policy.rs:53-110`. TOKEN_API 10: Host "нормализованный как authority и сверенный". `canonical_authority` returns `None` for `Example.com`, `host:80` on ws, `[2001:0DB8::1]`, so an incoming `Host: LOCALHOST` gets `400`, and `tests/policy_tests.rs:79-108` (which expects normalisation) fails. Config validation still needs "entries must already be canonical", which works with a normalising function via `canonical != entry`. Replacement:
    ```rust
    pub(crate) fn canonical_authority(value: &str, tls: bool) -> Option<String> {
        if value.is_empty()
            || value.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            || value.contains(['@', '/', '?', '#', '%'])
        {
            return None;
        }
        let (host, port) = if let Some(rest) = value.strip_prefix('[') {
            let (literal, suffix) = rest.split_once(']')?;
            let address = literal.parse::<Ipv6Addr>().ok()?;
            (format!("[{address}]"), parse_port(suffix)?)
        } else {
            let (host, suffix) = match value.find(':') {
                Some(at) => value.split_at(at),
                None => (value, ""),
            };
            let host = host.to_ascii_lowercase();
            let label_ok = |label: &str| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            };
            if host.len() > 253 || !host.split('.').all(label_ok) {
                return None;
            }
            (host, parse_port(suffix)?)
        };
        let default = if tls { 443 } else { 80 };
        Some(match port.filter(|port| *port != default) {
            Some(port) => format!("{host}:{port}"),
            None => host,
        })
    }

    /// `""` -> no port; `":N"` with 1 <= N <= 65535 -> `Some(N)`; anything else invalid.
    fn parse_port(suffix: &str) -> Option<Option<u16>> {
        if suffix.is_empty() {
            return Some(None);
        }
        let digits = suffix.strip_prefix(':')?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse::<u16>().ok().filter(|port| *port != 0).map(Some)
    }
    ```
    (`use std::net::Ipv6Addr;` at the top.) With this, `boot/config/tests/http_listener_tests.rs:544-584` and `tests/policy_tests.rs:79-108` both hold.
11. **Per-request config re-validation returns 500** - `route.rs:97-101` re-validates `origins` with the local `valid_origin` on every request. `http_listener.rs:134-139` validates the same list with `BrowserOriginPolicy::try_allow`, which accepts wildcard patterns (`*.example.com`, `transport-ws/src/browser.rs:115-118`). Such a config boots and then every native AND browser WS request answers `500`. Delete `:97-101` and `upgrade.rs:100-103` (`validate_allowlist`). Exact matching stays (TOKEN_API 10: "точный allowlist Origin без wildcard"); document that wildcard entries never match on `kind: http`.
12. **Limiter feeding is wrong** - `route.rs:174-181`: a first-frame defect or timeout closes `4000` without feeding the subnet limiter/aggregator (brief decision 11, TOKEN_API 5/10). `route.rs:185-186`: for `BrowserVerdict::Failed`, `verify_browser` already called `self.failure(..)` (`gate.rs:206-207`), and the route calls `record_browser_failure` again -> every credential failure is counted twice. Fix: call `self.gate.record_browser_failure(session.peer.clone())` in the `Err(())` arm of `read_browser_token`, remove it from the `Failed` arm.
13. **Activation failure codes** - `route.rs:211-215`: any activation error after a verified frame closes `4000` except `503 -> 1013`; TOKEN_API 10 requires `4011` for a revoke/owner-invalidation race at activation and `4012` for an expired token/session; `4000` is only for unverified failures. Also the `1013` close carries the reason "authentication_failed". Fix in B-r3 (typed `ActivationError`) and use `crate::framer::close_reason(code)` for every close reason (`:178`, `:188`, `:193`, `:198`, `:214`).
14. **First frame accepts trailing bytes and a `bin` token** - `first_frame.rs:73`: `rmp_serde::from_slice` does not check end of input (rmp-serde 1.3.1 `decode.rs:1171-1187`: `from_read_ref` deserializes and returns); `map.next_value::<String>()` also accepts msgpack `bin` (String's visitor takes valid UTF-8 bytes), and `next_key::<String>` accepts a `bin` key. Spec: exactly `{"token": str}`, no trailing bytes. Replace `StrictToken` and its `Deserialize` impl with a byte-exact parser:
    ```rust
    /// Decodes exactly a one-entry map `{"token": <str>}` with no trailing bytes.
    fn decode_token_map(payload: &[u8]) -> Option<String> {
        let rest = payload.strip_prefix(&[0x81])?;
        let (key, rest) = read_str(rest)?;
        if key != b"token" {
            return None;
        }
        let (value, rest) = read_str(rest)?;
        if !rest.is_empty() {
            return None;
        }
        std::str::from_utf8(value).ok().map(str::to_owned)
    }

    /// Reads one msgpack str (fixstr / str8 / str16 / str32); `bin` and every other type are rejected.
    fn read_str(input: &[u8]) -> Option<(&[u8], &[u8])> {
        let (&marker, rest) = input.split_first()?;
        let (len, rest) = match marker {
            0xa0..=0xbf => (usize::from(marker & 0x1f), rest),
            0xd9 => {
                let (&len, rest) = rest.split_first()?;
                (usize::from(len), rest)
            }
            0xda => {
                let (len, rest) = rest.split_first_chunk::<2>()?;
                (usize::from(u16::from_be_bytes(*len)), rest)
            }
            0xdb => {
                let (len, rest) = rest.split_first_chunk::<4>()?;
                (usize::try_from(u32::from_be_bytes(*len)).ok()?, rest)
            }
            _ => return None,
        };
        (rest.len() >= len).then(|| rest.split_at(len))
    }
    ```
    and at `:73-77`: `let token = decode_token_map(&payload).ok_or(())?; if token.len() != 71 || !token.is_ascii() { return Err(()); } return Ok(token);`. Drop the `serde` imports (`:5-6`).
15. **Pre-auth subnet race** - `pre_auth.rs:146-153`: the subnet counter is cloned under the DashMap entry guard but reserved after the guard is released; a concurrent last `Drop` (`:178-189`) can remove that entry in between, leaving the new permit on an orphaned counter and letting a fresh counter admit 8 more from the same subnet. Reserve while the entry guard is held:
    ```rust
    let counter = {
        let entry = self
            .subnet_counts
            .entry(subnet)
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
        if reserve(entry.value(), SUBNET_LIMIT).is_none() {
            drop(entry);
            self.global.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Arc::clone(entry.value())
    };
    ```
    (`Drop`'s `remove_if` takes the same shard lock, so it either runs before the reservation and the reservation creates a fresh entry, or after it and sees `load() != 0`.) Drop the `Entry` import. Add `#[cfg(test)] pub(crate) fn in_use(&self) -> (usize, usize)` (global, and the subnet count via `get`) for the "counters return to zero" unit test.
16. `PreAuthAdmission` is created per listener (`launcher.rs:158`); TOKEN_API says 256 "глобально". Either create one `Arc<PreAuthAdmission>` in `launch.rs` and pass it through a `ListenerEnv` field (both files are WP-B), or record the per-listener reading as a deviation in DESIGN.md. Low.
17. Native duplicate `Origin` -> `403` (`upgrade.rs:49-51`); browser checks Origin count and Host count together (`:58-60`), so a duplicated Host with an unlisted Origin yields `400`, not the `403` the brief's order implies. Low; split into two checks in brief order (Origin count -> Origin validity -> Origin allowlist -> Host count -> Host validity/allowlist).
18. `upgrade.rs:79` local `use base64::Engine;` (rust-guard LOCAL_USE, CLAUDE.md "imports at the top"); `route.rs:323` local `use tokio_tungstenite::tungstenite::protocol::{frame::coding::CloseCode, CloseFrame};`. Hoist both. S4 but trivially fixed with the rest.
19. `tests/policy_tests.rs:110-133`: "duplicate origin rejection" appends ONE `Origin` that is in the allowlist, so `validate_head` succeeds and `.err().expect(..)` panics. Append a second `Origin` header (`headers.append(ORIGIN, HeaderValue::from_static("https://app.example"))` twice).
20. Missing unit tests required by the brief (none exist): `first_frame` table (valid 80-byte frame; 256 accepted / 257 rejected before allocation; text, continuation, FIN=0, RSV, unmasked, 126-byte control, 9th control, close frame, 5 s timeout with `start_paused`, extra/repeated/missing key, trailing byte, `bin` token, non-string, length 70/72); pre-auth (8 per subnet, 256 global, release on drop, the race shape of item 15); Origin/Host table for the browser head; native Origin with empty vs non-empty allowlist; `response_headers` never contains `sec-websocket-extensions`; WS redaction (capture logs like `http_rest/tests/redaction_tests.rs` around `authenticate(.., Ws, ..)` and `verify_browser` for valid and invalid tokens; assert no token / secret substring).

Token loop (`src/sync/connection/token_ws/`):

21. **Teardown happens after the close frame** - `token_loop.rs:374-382`: `drop(cleanup)` (store removal, tx rollback, cursor removal, gauge) runs after `writer_task.finish()` has already sent the close frame; `WsSession`'s drop (`route.rs:66-72`) is later still. A client that sees `4012` and immediately checks `sessions().count_for_token(..)` races the teardown (`tests/e2e/http_ws/revocation.rs:112-119` is flaky by construction; `revoke`/`revoke_user` remove sessions themselves, so only the ttl case races). Brief 17 + "staged write is NOT visible after teardown" need the cleanup before the close. Order: `subscriptions.close_all(); dispatches.abort_all(); while dispatches.join_next().await.is_some() {} drop(cleanup); drop(connection); drop(sender); if !writer_done { writer_task.finish().await; }`.
22. Application errors return `{rid, error: "internal_error"}` (`token_loop.rs:363-368`); brief decision 15 / SCRAM `application_error_boundary` (`shamir-connect/src/server/infra/dispatch.rs:51-57`) use `application_error`. Use `"application_error"`; keep `internal_error` only for local encode failures.
23. Metrics missing (TOKEN_API 13, acceptance 11): no `shamir_token_request_denied_total{endpoint="ws"}` increment in the `HandleDenial::Forbidden` arm (`:343-355`), no `shamir_token_request_duration_seconds{endpoint="ws"}` around the handler call (`:321-327`). Add `metrics::counter!("shamir_token_request_denied_total", "endpoint" => "ws").increment(1);` and `let started = tokio::time::Instant::now(); .. metrics::histogram!("shamir_token_request_duration_seconds", "endpoint" => "ws").record(started.elapsed().as_secs_f64());`.
24. `token_loop.rs:330-333`: `byte_budget::take_stashed_serialized_response()` is called outside the `run_with_handoff` task-local scope, so it always returns `None`; delete it and use `response` directly.
25. Magic numbers: revalidation `Duration::from_secs(1)` (`:174`) -> `const TOKEN_WS_REVALIDATE_INTERVAL: Duration = Duration::from_secs(1);` (brief name); `16 * 1024 * 1024` (`:213`) -> `shamir_transport_ws::framing::MAX_WS_FRAME_SIZE`; `max_in_flight: 8` (`launcher.rs:171`) -> `shamir_tunables::instance_defaults::CONN_MAX_IN_FLIGHT` (what the SCRAM loop uses, `support/helpers.rs:95`).
26. `token_loop.rs:150` writer task via bare `tokio::spawn` (brief decision 3: no untracked spawn). It is aborted by `WriterTaskGuard` on drop, so the lifetime is bounded; acceptable if recorded in DESIGN.md, otherwise run it in a local `JoinSet`. Low.
27. `token_loop.rs:258-262` `.map(..).unwrap_or(true)` -> write `if !envelope.session_id_array().is_ok_and(|sid| sid == &ctx.session.session_id)`. Low.
28. `token_loop.rs:21` imports its own module through `crate::sync::connection::token_ws::{..}`; use `super::{TokenSessionTeardown, TokenWsFrameReader, TokenWsFrameWriter}`; `:28` use `shamir_transport_http::UpgradedIo` (crate-root re-export). Low.

SCRAM path (acceptance 12):

29. `session/writer_task.rs` dropped the explanatory comments that were in the moved code (DEFECT C fix, RI-15 budget-guard release, "Write error is ignored; the connection is closing"). Copy them back verbatim. S4.

e2e (`tests/e2e/http_ws/`) - see section 2 for coverage; wrong/weak oracles:

30. `native.rs:13-14` checks only the status of the valid upgrade; the binding frame and the decision-14 `expires_at_ns` value are never asserted for a native socket outside `session_loop.rs:57-69` (which checks `> 0` only). Assert `binding.expires_at_ns == created.expires_at_ns` (token TTL 3600 s, `session_max_age_hours` default is longer) - requires the harness to return the token record's expiry, or assert `expires_at_ns <= now + 3600 s + slack` and `> now`.
31. `native.rs:40-54` compares the `401` of two MISSING-header requests only; acceptance 2 requires identical `401` across missing / duplicate / malformed / unknown id / wrong secret / revoked / expired.
32. `max_sockets_per_token: 2` / `max_sockets_per_owner: 3` are configured (`native.rs:106-109`) but never exercised; no `503` + `Retry-After` case, no `429` limiter case, no "valid token still upgrades after 429".
33. `revocation.rs` has no live subscription, no owner role change, no tx-rollback / cursor / cancel-registry assertions (acceptance 8).

### 1.4 S4 - layout / rust-guard / hygiene

- `src/sync/connection/token_ws/tests.rs` is a single test file; CLAUDE.md requires a `tests/` directory with a manifest `mod.rs` (orchestrator step 0).
- `http_ws/tests/mod.rs:1` carries a `//!` line; TESTS_MANIFEST allows only `mod` declarations by the README wording ("not even `use`"). Remove it to be safe.
- Visibility widenings not listed in DESIGN.md: `proxy.rs:36` `resolved_peer` private -> `pub(super)` (not needed: no new caller; revert), `executor.rs:49` `ScopedHandler::handle` and `:74` `HandleDenial` -> `pub(crate)` (needed, list them), `accept.rs:24` (revert, A5).
- `deploy/server.example.ktav:87-108`: fine (commented example); also say that wildcard Origin entries never match on `kind: http`.
- DESIGN.md: several statements are stale (`route.rs:94 resolved.addr` - now `peer.addr = resolved` and correct; "launcher literal lacks `gate`" - it is present at `launcher.rs:154`; "`rmpv` at `token_loop.rs:244`" - gone); "Final report" appears twice; the first Acceptance 12 line has stray table pipes. Replace with one current report after the fixes.

### 1.5 Half-written files / leftovers / duplicates

- `sync/durability/tx_registry.rs:152-165`, `:311-326` - triplicated accessor, duplicated method (C1, C2).
- `sync/connection/token_ws/binding.rs:23-39` - dead `new_binding` written against a non-existent method (C6).
- `sync/connection/token_ws/framer.rs:29-46` - dead constructors (C12).
- `sync/connection/token_ws/token_loop.rs:131`, `:269`, `:330-333` - stale move, `drop(envelope)`, out-of-scope stash call.
- `security/perimeter/http_ws/route.rs:97-101` (config re-validation), `:261-262` (duplicate `now`), `:155-159` / `:204-208` (field moves out of a `Drop` type).
- `security/perimeter/http_ws/tests/policy_tests.rs` - written against an older `validate_head` signature (6 args) and against a normalising `canonical_authority` that was never implemented.
- `security/perimeter/http_rest/tests/scope_tests.rs:207-209` - `Filter::And` placeholder to silence an import.
- `shamir-transport-http/src/framing/tests/tunnel_tests.rs:52-78` - test without assertions.
- `shamir-transport-http/src/framing/guarded_io.rs:53-55` - unused helper.
- `shamir-transport-http/src/server/accept.rs:276-278`, `:290-297` - no-op churn (`let _ = result;`).

---

## 2. Acceptance status (dependency order)

Nothing has compiled; every item is at best PARTIAL until CI is green. "Code" = implementation present; "Tests" = oracle present.

| # | Item | Status | Present | Missing |
|---|---|---|---|---|
| 12 | SCRAM path unchanged | **PARTIAL (currently broken)** | Writer task extracted with identical arms (`session/writer_task.rs`), raw framing variants share the receive loop. | W1 duplicate attribute on an existing test; B17 existing maintenance test fails; S3-6/8/9 edited REST assertions/tests; S3-1 REST shutdown path change; C10 lint. |
| 1 | Native upgrade + binding | PARTIAL | `route.rs` admit -> activation before `101`, `upgrade.rs` accept key / `shamir-v1`, no extensions header (`service.rs:133-140`), binding `{ok,sid,expires_at_ns}` (`binding.rs`), deadline arithmetic (`binding.rs:42-47`). e2e `session_loop.rs::open_native` checks protocol, no extensions, exactly three keys, non-zero 32-byte sid. | Compile fixes (A1-A3, B1, B3-B6, C3-C6); exact `expires_at_ns` oracle (S3-30); unit test "binding bytes have exactly three keys" needs C7-C9. |
| 2 | Native refusals before `101` | PARTIAL | Order method -> head -> Origin -> gate -> cap -> `101`; uniform `401`; `403 token_channel_denied`; `400`; `405 + Allow`; Origin `403`; `503 + Retry-After` (`route.rs:281-285`). e2e: malformed/duplicate/missing `401`, channel `403`, protocol/key `400`, `405`, Origin `403` with empty list. | Identical-`401` matrix incl. unknown id / wrong secret / revoked / expired (S3-31); cap `503` and limiter `429` + valid-token-still-upgrades (S3-32); Origin vs non-empty allowlist; gate unit test B15 is not even mounted. |
| 3 | Plaintext rails | PARTIAL | `gate.rs` rail for `Endpoint::Ws` and browser verdict; `api_token_exposed` with `channel = ws`. | Native WS `403 token_plaintext_write_denied` test + exposure audit assertion (unit level in `gate_tests.rs` is enough); REST exposure tuple regression (S3-6). |
| 4 | Browser route before `101` | PARTIAL | Availability `404` (`route.rs:92-94`, launcher `:160`), Origin/Host checks, pre-auth permits `503`. e2e: missing/`null`/unlisted/malformed Origin, duplicate Host. | Normalisation (S3-10), race (S3-15), 500 on wildcard configs (S3-11); tests for duplicate Origin, missing/malformed/unlisted Host, `404` on plaintext non-loopback (unit-test the availability predicate), pre-auth caps, "no frame before verification". |
| 5 | Browser first frame | PARTIAL | Hand parser with bounds-before-allocation, 8 controls, 5 s deadline, `4000/4013/4014/1013` mapping, `WebSocketStream::from_raw_socket` with 16 MiB config. e2e: one valid frame -> binding. | Trailing bytes / `bin` (S3-14), limiter feeding (S3-12), codes and reason (S3-13); the whole first-frame table (S3-20); permit release assertion. |
| 6 | Session loop | PARTIAL | `token_loop.rs` dispatch over `ScopedHandler::handle(.., Endpoint::Ws, ..)`, `{rid,res}` / `{rid,error}` / `{error:"invalid_envelope"}`, scope denials inside `res`, post-auth rate limit, alias caps, per-request recheck. Scope WS column in `scope.rs`. e2e: `Ping`, read-only write `Execute` -> `token_read_only`. | Compile fixes C3-C5, C8; application_error (S3-22); metrics (S3-23); e2e for read/write `Execute`, `TxBegin/TxExecute/TxCommit`, cursors, `Subscribe` + push frame shape, admin and database denials, reply-shape exactness. |
| 7 | `sid` isolation | PARTIAL | `token_loop.rs:258-266` closes `4010` without reply; SCRAM store never sees token sessions by construction. | Tests: other connection's token sid, SCRAM sid, all-zero sid -> `4010`; token sid over a SCRAM connection -> `session_expired`. |
| 8 | Revocation / expiry / teardown | PARTIAL | `TokenWsCancelRegistry` + `with_canceller` in `launch.rs:578-600`, register-then-recheck (`route.rs:286-302`), 1 s revalidation, deadline arm, `TokenSessionTeardown` (store, tx via `remove_for_session`, cursors via `remove_all_for_session`, gauge). e2e: revoke -> `4011`, ttl 2 s -> `4012`, `revoke_user` -> `4011`, session counts. | C1/C2; teardown order (S3-21); subscription-live case, owner role change, staged-write-not-visible, cursors gone, registry entry gone (S3-33). |
| 9 | Text `1003`, oversize `1009`, heartbeat `4003`, shutdown `1000` | PARTIAL | Error mapping in `token_ws/framer.rs:85-110`, shared writer heartbeat, shutdown token -> `1000`. e2e `shutdown.rs`. | Tests for `1003`/`1009` (e2e: send `Message::Text` / a 16 MiB + 1 binary frame after binding) and a heartbeat unit test over the token framer (`tokio::time::pause`, no Pong -> `4003`). |
| 10 | Token sockets not counted against SCRAM cap | PARTIAL (design only) | Separate `TokenSessionStore`. | Test: configure `max_sockets_per_owner` >= N, open N token sockets, then a native SCRAM login of the owner still succeeds (fits launch (a)). |
| 11 | Metrics / audit / redaction | PARTIAL | `auth_total{channel="ws"}` pre-registered and counted; `active_ws` gauge; channel-aware failure summaries. | `request_denied_total{endpoint="ws"}` / `request_duration_seconds{endpoint="ws"}` (S3-23); WS redaction test (S3-20); REST tuple regression (S3-6). |

---

## 3. Finishing plan (3 disjoint work packages, about 2 hours each)

### Step 0 - orchestrator, before dispatching (5 minutes; children may not move or delete files)

```
cd crates/shamir-server/src/security/perimeter/http_ws
mkdir browser
mv browser_policy.rs browser/policy.rs
mv pre_auth.rs browser/pre_auth.rs
mv first_frame.rs browser/first_frame.rs
cd ../../../sync/connection/token_ws
mkdir tests
mv tests.rs tests/binding_tests.rs
```
Create `crates/shamir-server/src/sync/connection/token_ws/tests/mod.rs` with the single line `mod binding_tests;`.
Create `crates/shamir-server/src/security/perimeter/http_ws/browser/mod.rs`:
```rust
//! Browser-route Origin/Host policy, pre-auth permits and first-frame parsing.

pub(crate) mod first_frame;
pub(crate) mod policy;
mod pre_auth;

pub(crate) use pre_auth::{PreAuthAdmission, PreAuthPermit};
```
Replace `crates/shamir-server/src/security/perimeter/http_ws/mod.rs` with:
```rust
//! WebSocket security and admission policy for HTTP listeners.

pub(crate) mod browser;
pub(crate) mod cancel_registry;
mod route;
#[cfg(test)]
mod tests;
mod upgrade;

pub(crate) use browser::PreAuthAdmission;
pub(crate) use route::WsUpgradeService;
```
Result: `http_ws/` 6 entries, `http_ws/browser/` 4, `token_ws/` 6, `token_ws/tests/` 2.

Every child brief carries the CLAUDE.md compile policy verbatim (only `cargo fmt -p <crate>` / `-- --check`, `cargo check -p`, `cargo clippy -p ... -- -D warnings`, `python -I scripts/ci/check_layout.py` if the user allows them; otherwise none) and the git-mutation ban verbatim.

Cross-package contracts (do not change): `token_ws` re-exports `encode_binding(&[u8; 32], u64) -> Result<Vec<u8>, ()>`, `generate_sid() -> Result<[u8; 32], ()>`, `session_deadline(u64, u64, u64) -> u64`, `run_token_loop(TokenLoopContext, WebSocketStream<UpgradedIo>)`, `TokenLoopContext { service, handler, grant: Arc<Verified>, session, expires_at_ns, cancel, shutdown, max_in_flight, ping_interval, pong_timeout }`; `crate::http_rest::{HandleDenial, ScopedHandler}` stay crate-visible with `ScopedHandler::handle(&self, &Session, DbRequest, Endpoint, &ConnectionServices)`; `crate::framer::close_reason` unchanged.

### WP-A - transports (shamir-transport-http, shamir-transport-ws, revert in shamir-connect); about 60-75 min

Files: `crates/shamir-transport-http/src/server/{accept.rs,service.rs}`, `src/server/routes/ws/{mod.rs,hook.rs}` + new `src/server/routes/ws/handler.rs`, `src/framing/guarded_io.rs`, `src/framing/tests/tunnel_tests.rs`, `src/server/tests/ws_route_tests.rs`; `crates/shamir-transport-ws/src/tests/framing_tests.rs`; `crates/shamir-connect/src/server/auth/token/mirror.rs`.

1. A1, A2, A4 (service.rs imports / `mut request`).
2. A5, A6 (BoxedIo, `GuardedIo::into_inner`).
3. S3-1 restore the REST completion path and master text in `accept.rs`.
4. A3, A7 (`ws_route_tests.rs` path and `async fn` gate/executor).
5. W1 (duplicate attribute).
6. S3-5 revert `mirror.rs` to master.
7. S3-2 move `handle_ws` + `ws_rejection` to `routes/ws/handler.rs` (`pub(crate) async fn handle_ws`), `service.rs` keeps the path match and `return handler::handle_ws(request, context, route).await;`; widen `auth_header`, `retry`, `unauthorized_body` to `pub(crate)`; note the widenings for DESIGN.md.
8. S3-3, S3-4 tests (tunnel oracle + idle-deadline test; ws_route: browser 404 via `NoWs`, 405, hook rejection with `Retry-After`, extension header stripped, shutdown cancels the `run` token).
9. Read every changed file end to end; `cargo fmt -p shamir-transport-http -p shamir-transport-ws -p shamir-connect`.

### WP-B - server security perimeter, config, launcher, browser e2e; about 120 min

Files: `crates/shamir-server/src/security/perimeter/http_rest/{gate.rs,executor.rs,scope.rs,maintenance.rs,mod.rs}`, `http_rest/tests/{gate_tests.rs,scope_tests.rs,maintenance_tests.rs,redaction_tests.rs,executor_tests.rs}`, everything under `crates/shamir-server/src/security/perimeter/http_ws/` (incl. `browser/`, `tests/`), `crates/shamir-server/src/boot/config/{http_listener.rs,tests/http_listener_tests.rs}`, `crates/shamir-server/src/engine/server/server_launcher/http/{launcher.rs,proxy.rs}`, `crates/shamir-server/src/engine/server/server_launcher/{launch.rs,listeners.rs}` (only if item 16 is taken), `crates/shamir-server/tests/e2e/http_ws/browser.rs`, `deploy/server.example.ktav`.

1. Path updates after step 0: `route.rs:21-22` -> `use super::browser::first_frame::read_browser_token; use super::browser::{PreAuthAdmission, PreAuthPermit};`; `upgrade.rs:8` -> `use super::browser::policy::{host_allowed, valid_origin};` (drop `canonical_authority` with `validate_allowlist`); `tests/policy_tests.rs:3-4` -> `super::super::browser::policy::..`, `super::super::browser::first_frame::FIRST_FRAME_MAX`; `boot/config/http_listener.rs:6` -> `crate::http_ws::browser::policy::canonical_authority` (B2). Remove the `//!` line from `http_ws/tests/mod.rs`.
2. B12 (`http_rest/mod.rs` re-exports), B13 (collapsible_if), B14-B16 (test imports, missing `#[tokio::test]`), B7-B8 (`scope_tests.rs`), then S3-8 (restore REST tests from master + `Endpoint::Rest`, exhaustive `expected_ws`).
3. S3-6, S3-7, B17, S3-9 in `maintenance.rs` / `maintenance_tests.rs` (REST tuple, warn fields, comments, original layout of untouched `select!` arms, master log strings).
4. Browser policy: S3-10 (normalising `canonical_authority` + `parse_port`), S3-15 (pre-auth reservation under the entry guard, `in_use` test accessor), S3-14 (`decode_token_map`/`read_str`), B23.
5. `upgrade.rs`: B21, B22, delete `validate_allowlist`, hoist `use base64::Engine;` (S3-18), split the Origin/Host checks in brief order (S3-17), fmt.
6. `route.rs` (B-r3, covers B3, B19, B20, S3-13):
   ```rust
   enum ActivationError { Busy, Revoked, Expired }

   impl ActivationError {
       fn reject(&self) -> WsReject {
           match self {
               Self::Busy => reject(StatusCode::SERVICE_UNAVAILABLE, Some(1)),
               Self::Revoked | Self::Expired => reject(StatusCode::UNAUTHORIZED, None),
           }
       }
       fn close_code(&self) -> u16 {
           match self {
               Self::Busy => 1013,
               Self::Revoked => 4011,
               Self::Expired => 4012,
           }
       }
   }

   /// Admits `grant` into the token store and fills `target`; `target`'s Drop removes the store entry on every later exit.
   fn activate(&self, grant: Verified, target: &mut WsSession) -> Result<(), ActivationError> {
       let now = now_ns();
       if now == u64::MAX {
           return Err(ActivationError::Busy);
       }
       let sid = generate_sid().map_err(|()| ActivationError::Busy)?;
       let record = grant.state.record();
       let deadline = session_deadline(now, record.expires_at_ns, self.session_max_age_hours);
       if deadline <= now {
           return Err(ActivationError::Expired);
       }
       let session = Session::for_token(&grant, (self.username)(&record.owner_user_id), now, sid);
       let session = self.service.sessions().admit(sid, session, self.caps)
           .map_err(|_| ActivationError::Busy)?;
       target.session = Some(session);
       let registration = self.registry.register(TokenId(record.id));
       let cancel = registration.token();
       target.registration = Some(registration);
       if cancel.is_cancelled() || !self.service.still_valid(&grant, now) {
           return Err(if now >= record.expires_at_ns { ActivationError::Expired } else { ActivationError::Revoked });
       }
       target.cancel = Some(cancel);
       target.deadline_ns = deadline;
       target.grant = Some(grant);
       target.permit = None; // releases the browser pre-auth permit on success
       Ok(())
   }
   ```
   (rename `_registration` to `registration`; borrow `record` only before `grant` is moved, i.e. copy `record.id`/`expires_at_ns`/`owner_user_id` into locals first if the borrow checker complains). `admit`: `self.activate(grant, &mut session).map_err(|error| error.reject())?;`. `run`: `async fn` (B18); first-frame `Err(())` arm feeds `record_browser_failure` (S3-12); `Failed` arm does not; activation `Err(error)` closes `error.close_code()`; every close uses `crate::framer::close_reason(code)`; `TokenLoopContext.grant: Arc::new(Verified { state: Arc::clone(&grant.state) })` (B4); delete `:97-101` (S3-11); hoist the `CloseCode`/`CloseFrame` import.
7. B24 (`cancel_registry.rs`), B10 done by step 0, B25, S3-19.
8. Launcher: B5, B6, `max_in_flight: CONN_MAX_IN_FLIGHT` (S3-25), annotate `move |peer: &PeerInfo| super::proxy::resolve_peer(peer, &trusted)` (+ `use shamir_transport_http::PeerInfo;`), revert `proxy.rs:36`; optional S3-16.
9. Unit tests S3-20 in `http_ws/tests/` (new `first_frame_tests.rs`, `pre_auth_tests.rs`; `tests/` then has 5 entries), plaintext-rail native WS test in `gate_tests.rs`, WS exposure test in `maintenance_tests.rs`.
10. e2e `browser.rs` (launch (b)): add duplicate Origin, missing/malformed/unlisted Host, a `rest`-only token -> `4013`, an invalid token, a 257-byte frame, a text frame and a trailing-byte frame -> `4000` with identical reason, a frame-less socket closed `4000` after the 5 s timeout (wait for the server close, bounded by 15 s), and with `max_sockets_per_token` set to 1 via the hook a second browser socket of the same token -> `1013`.
11. `cargo fmt -p shamir-server` (coordinate: only one package should run fmt on shamir-server at the end; WP-B runs it last, after WP-C is done, or the orchestrator runs it once).

### WP-C - server sync/engine, token loop, remaining e2e; about 120 min

Files: `crates/shamir-server/src/sync/connection/{request_loop.rs,session/writer_task.rs}`, everything under `crates/shamir-server/src/sync/connection/token_ws/` (incl. `tests/`), `crates/shamir-server/src/sync/durability/tx_registry.rs`, `crates/shamir-server/src/sync/framer.rs`, `crates/shamir-server/src/engine/cursor_registry.rs`, `crates/shamir-server/src/tests/engine/connection/ws/framer_close_tests.rs`, `crates/shamir-server/tests/e2e/http_ws/{mod.rs,common.rs,native.rs,session_loop.rs,revocation.rs,shutdown.rs}`, `crates/shamir-server/tests/e2e/http_rest/common.rs`.

1. C1, C2 (`tx_registry.rs`), C10 (`request_loop.rs`), S3-29 comments in `writer_task.rs`.
2. `token_ws`: C6 (+ `mod.rs` re-export line becomes `pub(crate) use binding::{encode_binding, generate_sid, session_deadline};`), C11, C12, C3, C4, C5, C13, C14, S3-21 teardown order, S3-22, S3-23, S3-24, S3-25 constants, S3-27, S3-28; C8 visibility for the tests.
3. `token_ws/tests/binding_tests.rs`: C9, C15 (paths are already right after step 0). Add `token_ws/tests/framer_tests.rs` (mount in `tests/mod.rs`): prefix strip (`write_frame_prereserved` over a duplex `WebSocketStream` sends exactly the payload), wrong declared length -> `FramerError::Decode`, text message -> `UnsupportedData` (close intent `1003`), oversized message -> `TooLarge` (`1009`); heartbeat: `run_writer_task` with a paused clock and no Pong closes `4003`.
4. `framer_close_tests.rs`: assert `close_reason` for `1013, 4000, 4010, 4011, 4012, 4013, 4014`.
5. e2e harness: E1; E2.
6. e2e launch (a) `native.rs` + `session_loop.rs` (keep the limiter case last): S3-30, S3-31 (create tokens for revoked/expired cases via `token_service.revoke` and `token_with_ttl(.., 1)` waiting for expiry through a server-close-free method: create with ttl 1 s and use it after the `ttl_secs = 2` socket of launch (c)? simpler: revoked token + `token_with_ttl` whose expiry is reached by opening a socket that the server closes `4012`, then reuse its string for the 401 comparison), caps `503` + `Retry-After` with 2 sockets of one token and a third, owner cap with 3 sockets over two tokens, read and write `Execute` with a `read_write` token on `items`, `TxBegin`/`TxExecute`/`TxCommit`, `CreateCursor`/`FetchNext`/`CancelCursor`, `Subscribe` + a write on another socket -> push frame with keys `push, sub, seq`, admin request -> `token_request_denied`, token with `databases: ["other"]` -> `token_database_denied` (needs a `token_with_options` variant taking `databases`), `sid` isolation (`4010` for another socket's sid / all-zero sid), token sid over the native SCRAM connection -> `session_expired`, text frame -> `1003`, 16 MiB + 1 frame -> `1009`, accounting (owner's SCRAM login succeeds while token sockets are open), then failures until `429` and a valid token still upgrades.
7. e2e launch (c) `revocation.rs`: subscription live then `revoke` -> no push and `4011`; open `TxBegin` + staged `TxExecute` write before a close and assert the row is not visible afterwards through the native session; cursor gone (`FetchNext` from a new socket of the same token with the old cursor id -> error); owner role change through the native admin session -> `4011` within the revalidation interval (wait for the close, 15 s bound). After S3-21 the session-count assertions are race-free.
8. Read every changed file end to end.

---

## 4. What this review could not verify

- Anything at runtime: hyper 1.9 upgrade hand-off through `GuardedIo` (whether hyper polls the IO for a pipelined request between reading the upgrade head and writing `101`, and how the GuardedIo state machine handles that read), tungstenite close-frame delivery after a capacity error, timing of teardown vs close frames, Windows behaviour of the e2e tests.
- Exact clippy verdicts that depend on lint internals rather than on syntax: `manual_async_fn` with a `+ Send` bound (expected to fire, consistent with this program's CI history), `needless_return` inside an `async move` block (expected), `private_interfaces` for a `pub(crate)` type alias over a module-private trait (A5) and for `RestHandler::new` (B12) (expected; both fixes are free reverts), `drop_non_drop` on `RequestEnvelopeView` (expected).
- Whether the launcher's `Arc::new({ ..; move |peer| .. })` closure gets a higher-ranked signature from the field type; the same pattern already compiles at `launcher.rs:77`, so the annotation is only hardening.
- Whether `tokio-tungstenite` 0.30's client (`client_async`) accepts the server's `101` when the request carried `Sec-WebSocket-Protocol` (the response echoes it, so it should).
- The JS client end to end (`crates/shamir-client-ts`); only its close-code mapping and the Node socket options were read.
- `rustfmt` results were taken with `rustfmt 1.9.0 --check --edition 2021` per file; `cargo fmt` uses the same rustfmt but crate-wide, so files not touched here were not re-checked.
- Line counts and folder entries were counted with `wc -l` / `ls -A` on the working tree; `check_layout.py` was not run (it ignores untracked files anyway).
