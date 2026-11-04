# T9 - handshake x transport matrix for all clients (design)

Base: master `03f7a2bda`. Read-only design by the `oh` reviewer plus orchestrator decisions. Line numbers refer to that base.

## 0. Principles

1. The server knows only the protocol, never the client. "Browser" variants are protocol profiles for platform limits (no TLS exporter, no custom WebSocket headers); any client may use them.
2. Two independent axes: **handshake** (session = SCRAM + resume; token; password-login -> token) and **transport** (TCP+TLS, TCP plain on loopback, WS, HTTP, local IPC; QUIC does not exist in code, mTLS does not exist in code).
3. Every client supports every cell the server protocol admits, except cells physically impossible on its platform (browser: no TCP, no IPC, no TLS exporter, no `Authorization` header on WebSocket).
4. Before any feature work, the layers are separated by a behaviour-preserving pre-refactor (T9-1, T9-1b). Out of T9: JSON bodies for REST (REST is MessagePack only) and mTLS.

## 1. Verified matrix

### 1.1 Server listeners

| Listener | binding_mode | Handshake | Code |
|---|---|---|---|
| `tcp`, `tls_exporter` | 0x01 | session, resume | `listeners.rs:250-254` |
| `tcp`, `plain` (loopback only) | 0x00 | session, resume | `listeners.rs:255-257`, `config.rs:731-737` |
| `ws`, `tls_exporter`, path `/shamir/v1` | 0x01 | session, resume | `listeners.rs:258-262` |
| `ws`, `tls_no_export`, path `/shamir/v1/browser`, Origin required | 0x02 | session, resume | `listeners.rs:263-270`, `config.rs:740-746` |
| `unix` (Unix socket / Named Pipe), `plain` | 0x00 | session, resume | `listeners.rs:220-235` |
| `http`: `POST /shamir/v1/request`, `GET /shamir/v1/ping` | - | token | `service.rs:87-89` |
| `http`: WS `/shamir/v1/ws` (Authorization header) | - | token | `http_ws/route.rs:131-135` |
| `http`: WS `/shamir/v1/ws/browser` (token in the first frame) | - | token | `browser/first_frame.rs` |
| `http`: `/shamir/v1/auth/begin`, `/finish` | - | password -> token | `service.rs:53` |

The profile belongs to the listener: the client must send the `binding_mode` of the listener (`handshake.rs:239-256`, before Argon2).

### 1.2 Cells outside the protocol (intentional)

- token x TCP / WS(`kind: ws`) / IPC: `TOKEN_API.md:22` ("listener `kind: tcp|ws|unix` does not accept API tokens"); a self-signed certificate on those listeners is justified only by the session Ed25519 pin, which a token client does not have.
- session x HTTP: `ADMIN_UI_HOSTING.md:7`, `TOKEN_LOGIN_HTTP.md:3`; SCRAM is a duplex exchange bound to a connection.
- QUIC and mTLS: not in code.

### 1.3 Client support today

See the draft table in section 5 (also the source of the documentation page).

## 2. T9-1 pre-refactor (behaviour unchanged)

Model: three layers and one link. **Transport** dials, does TLS, frames bytes/messages and yields `ChannelBinding {transport_kind, binding_mode, exporter[32]}`. **Handshake** works only on frames + binding (SCRAM, resume, bearer-WS, HTTP login) and never opens sockets. **Mux** (rid demux, push, lifecycle) sits on frames.

### 2.1 Rust `crates/shamir-client`

Findings: wire structs `WireAuthInit..WireResumeOk` live in `client/transport.rs:617-718`; the SCRAM code is duplicated in `connect.rs:396-485` (TCP) and `531-600` (IPC); resume works only over TLS (`connect.rs:638-678`).

Boundary ("behaviour does not change"): public signatures byte for byte - `Client::connect`, `connect_host`, `connect_local`, `resume`, `resume_secure`, the option structs (public fields, no `non_exhaustive`, so no field may be added), `bound_until`, `resolve_host`, `ClientError`, `CloseCause`, getters in `api.rs`, re-exports in `lib.rs`. Frame bytes, error texts (`"send auth_init"`, `"read challenge"`, ...), timeout phase names, no `handshake_timeout` for `connect_local`, Argon2 in `spawn_blocking`, `Zeroizing` of password and ticket, the ticket is sent only after the resume proof. Guard tests are NOT edited (`git diff --stat` on them is empty): all of `src/tests/**`, `client/subscription/tests`, `tests/{smoke.rs,smoke_local.rs,hostname,batch,cursor}`, the server e2e that use `shamir_client`, `examples/connect.rs`, `benches/roundtrip.rs`. Tests import `crate::client::transport::{Wire*, HookPoint, Lifecycle, ClosedState, await_pending_response, connect_with_timeout}` and `crate::client::{reader_task, ShutdownGuard, ...}`: keep those paths via re-exports.

Target layout (`client/` keeps at most 7 entries; `shamir-client/src` and `src/tests` are already at 7 - no new entries there):

```
client/
  mod.rs         re-exports only (public and test paths unchanged)
  api.rs, cursor_stream.rs, subscription/   unchanged (api.rs: write_prereserved + shutdown call sites)
  transport/     AXIS 1 (replaces transport.rs; path crate::client::transport kept)
    mod.rs binding.rs frame_io.rs dial.rs        [T9-2 ws.rs] [T9-3 http.rs, bearer_ws.rs]
  handshake/     AXIS 2
    mod.rs wire.rs scram.rs auth_ok.rs resume.rs [T9-3 bearer.rs] [T9-4 login.rs]
  session/       Client = transport x handshake x mux
    mod.rs client.rs entry.rs options.rs mux/(lifecycle.rs reader.rs hooks.rs)
```

Interfaces (`Client` stays one concrete type; the writer is an enum, the reader a generic parameter of the reader task, as today):

```rust
pub(crate) struct ChannelBinding { transport: TransportKind, mode: BindingMode, exporter: [u8; 32] }
pub(crate) trait FrameRead: Send + Unpin + 'static { /* read_frame_into(max, buf) */ }
impl<T: AsyncRead + Unpin + Send + 'static> FrameRead for T {}   // keeps reader_task(duplex, ..) tests unchanged
pub(crate) enum WriteSink { Tls(..), Ipc(..), #[cfg(test)] Test(..) }  // T9-2: Ws, T9-3: BearerWs
// methods write_frame / write_prereserved / shutdown
pub(crate) struct Dialed<R: FrameRead> { read: R, write: WriteSink, binding: ChannelBinding }
pub(crate) async fn run_scram<R: FrameRead>(d: &mut Dialed<R>, creds, phase) -> Result<HandshakeOutcome, ClientError>;
pub(crate) async fn resume_wire_exchange<R: FrameRead>(d: &mut Dialed<R>, ticket, pin) -> Result<ResumedHandshake, ClientError>;
// binding_mode and exporter come from d.binding, not from constants (connect.rs:738)
```

PR steps (each passes CI alone). **Order amendment (orchestrator):** R2 first, then R1, then R3, because `client/` already holds exactly 7 entries (`api.rs connect.rs cursor_stream.rs mod.rs options.rs subscription/ transport.rs`): R2 removes `connect.rs`/`options.rs` and adds `session/`, which frees the slots for `handshake/` (R1) and `transport/` (R3, replacing `transport.rs`). Briefs: `docs/dev-artifacts/prompts/t9/`.
- **R1 handshake:** create `handshake/` (wire, auth_ok, resume, scram) on top of `AsyncRead/AsyncWrite`; remove the SCRAM duplicate; add a unit test under `src/tests/wire/` (SCRAM over `tokio::io::duplex` with an emulated server for bindings (Tcp, TlsExporter, exporter) and (Unix, None, zeros); must not be `cfg(unix)`).
- **R2 session and mux:** create `session/` (client, entry, options, mux); old paths re-exported.
- **R3 transport seams:** `transport/{binding,frame_io,dial}.rs`; `WriteSink` loses `impl AsyncWrite` and gets methods; `reader_task` and `assemble` take `R: FrameRead`; dial returns `ChannelBinding`.

Checks without local runs: CI only (draft = fast tier; `[ci full]` marker alone on a line of the head commit = full tier). Manual diff checks: guard tests untouched, `pub fn|pub struct` lines in `lib.rs` and options unchanged, no `Cargo.toml` change. Registering a rust-guard conservation campaign is optional; start without it and add one only if the `structure` gate cannot accept the moves.

### 2.2 TypeScript `crates/shamir-client-ts`

Findings: `connect` and `resume` are hard-wired to `wss://.../shamir/v1/browser` (`client.ts:165, 285`); `runHandshake` defaults to `TRANSPORT_KIND_WS` and `BINDING_MODE_TLS_NO_EXPORT` (`protocol.ts:105-106`); `connectLocal` duplicates `connect` (`client.ts:219-267`); `changePassword` hard-codes the browser binding (`client-internals.ts:428-440`) while the server builds `auth_message_cp` from the real session parameters (`changepw.rs:140-150`) - after `connectLocal`, a password change very likely fails with `AuthFailed` (read from code, unproven; fix it in S4 with a red e2e first). Layout: `client.ts` is 993 lines; `src`, `core`, `core/client`, `__tests__`, `__tests__/connection` are at 7 entries.

Boundary: exports of `index.ts` and `browser.ts`, signatures of `connect`, `connectLocal`, `resume`, the static `ShamirClient.connect/connectLocal/resume(platform, opts)`, types `ConnectOptions`, `ConnectLocalOptions`, `ResumeOptions` stay. The token path (`entry/*`, `platform/token/*`, `core/protocol/token/*`, `login/*`) is not touched; its tests staying green is the proof. Guard tests: `__tests__/connection/{connect,resume,client-demux}.test.ts`, `core/client/__tests__/{client,connect-local}.test.ts`, `platform/__tests__/{node-ipc-socket,ws-subprotocol}.test.ts`, `platform/connect/__tests__/public-connect.test.ts`, `core/protocol/__tests__/*`; e2e: `e2e-connect-local`, `e2e-resume`, `e2e-subscriptions`, `auth/e2e-change-password`, data/query e2e.

Target interfaces (new `core/protocol/session/`, `platform/session/`):

```ts
interface ChannelBinding { transportKind: 0x01|0x02|0x03; bindingMode: 0x00|0x01|0x02; exporter: Uint8Array /*32*/ }
type SessionEndpoint =
  | { kind: 'ws-browser'; url; origin; tls } | { kind: 'ws-native'; url; tls }   // native: Node only
  | { kind: 'tcp'; host; port; tls } | { kind: 'ipc'; path };                    // Node only
interface SessionLink { socket: Socket; binding: ChannelBinding }
interface SessionDialer { dial(ep: SessionEndpoint, timeoutMs: number): Promise<SessionLink> } // unsupported -> 'unsupported_on_platform'
type SessionHandshake = { kind: 'scram'; username; password } | { kind: 'resume'; ticket; serverPubKey };
function openSession(p: Platform, link: SessionLink, hs: SessionHandshake): Promise<SessionOpened>; // data for the constructor, no import cycle
```

`Platform` keeps only crypto; `openSocket` and `openIpcSocket` become details of the dialers. `NodeIpcSocket` generalises to `NodeStreamSocket` (old export kept).

PR steps: **S0** move `__tests__/connection/{connect,resume,e2e-connect-local,e2e-resume}` into `connection/session/` (pure move; frees slots); **S1** `ChannelBinding`, `SessionEndpoint`, `SessionLink` types, `runHandshake` takes a binding, `ShamirClient` stores it, unit test on `auth_init` fields for browser and ipc; **S2** move bodies of `connect/connectLocal/resume` into `session/open.ts`, `session/resume.ts` (client.ts ~760 lines); **S3** `platform/session/{node-dialer,browser-dialer}.ts`; **S4** (a fix, not refactor) `changePassword` uses the session binding (red e2e over `connectLocal` first). TS e2e are outside the merge gate: run `gh workflow run ci.yml --ref <branch>` and `gh workflow run ts-e2e-nightly.yml --ref <branch>` per TS PR.

### 2.3 napi

Unchanged in T9-1: option mapping, secret protection (`secrets.rs`), promises and errors only; no protocol logic. Guard: `cargo check -p shamir-client-node` and the generated-file drift check in CI.

## 3. T9-2 .. T9-6

**T9-2 Rust session x WS and TCP plain.** TLS 1.3. Native: `TlsTrust::AppAuthenticated`. Browser profile 0x02 has no channel binding: it REQUIRES `WebPki` trust. Upgrade `GET <path>` with `Sec-WebSocket-Protocol: shamir-v1` (mandatory; 400 otherwise, wrong path 404). Native: no Origin; browser profile: Origin required and in the allowlist. Exporter label `EXPORTER-ShamirDB-AUTH-v1`, empty context, 32 bytes (same as TCP, so tickets move between TCP and WS). Frame = one BINARY message `[u32_be len][msgpack]`, `len` equals body length minus 4; TEXT is an error; 16 MiB limit, 4 KiB before auth. The server pings and closes with 4003 without a pong: the reader must keep polling. Implementation: `tokio_tungstenite::client_async_with_config` over an existing `TlsStream`, exporter taken before the upgrade; `ReadSource::Ws`, `WriteSink::Ws`; new `Client::connect_ws(WsConnectOptions)` (profiles `Native` / `Browser {origin}`), resume over WS, `Client::connect_plain` (loopback, 0x00). `shamir-transport-ws` as a path dependency, no new crates.

**T9-3 Rust token x REST and token x WS.** REST: `POST /shamir/v1/request`, exactly one `Authorization: Bearer shk1....`, `Content-Type: application/msgpack`, body = msgpack `DbRequest`; `GET /shamir/v1/ping`; HTTP/1.1 only, TLS 1.3 with ALPN `http/1.1`, mandatory chain and hostname verification, no redirects/cookies/query credentials. Statuses 400/401(`WWW-Authenticate: Bearer`)/403/404/405/408/413/415/429/503. No TxBegin/Execute/Commit/Rollback, cursors or subscriptions over REST (`not_supported_over_rest`): a separate `TokenHttpClient` (`ping`, `request`, `execute`, `close`). Token WS native route: `GET /shamir/v1/ws` with Authorization and exactly one `Sec-WebSocket-Protocol: shamir-v1`; the server first sends `{ok:true, sid:bin32, expires_at_ns}`; then each BINARY message is a raw msgpack envelope `{sid, rid, req}` without the u32 prefix (`BearerWs` strips it in `write_prereserved`). Close codes: 4003 heartbeat, 4010 wrong sid, 4011 token revoked, 4012 expired, 1013 busy; add `CloseCause::PeerClosed {code, reason}`. Secrets: `ApiToken(Zeroizing<String>)` validated by `shamir_connect::server::auth::token::format`, redacted `Debug`, `HeaderValue::set_sensitive(true)`, never in errors or tracing. HTTP client: decision deferred to the T9-3 brief (see section 6).

**T9-4 Rust password login -> token.** `POST /shamir/v1/auth/begin` then `/finish` (`TOKEN_LOGIN_HTTP.md` 1.1-1.2); transcript/scope from `shamir_connect::server::auth::http_login::{build_transcript, encode_scope, build_server_signature}`. The client verifies the server: expected `security` class derived from the URL and out-of-band policy (direct HTTPS 0x01, HTTPS via proxy 0x02, loopback HTTP 0x00; never trust the class from the challenge alone); `listener_id` pin if configured; Argon2id within client limits; constant-time `HMAC(server_key, T) == server_signature` BEFORE returning the token (on mismatch wipe the token and fail); the returned `id` must equal the id inside the token. No Ed25519 pin: TLS authenticates the server.

**T9-5 napi.** Expose `connectLocal`, `resume` (via `resume_secure`, ticket as Buffer), `connectWs`, `connectTokenWs`, class `ShamirTokenHttpClient`, `login()`, getter `authKind()`. Split `lib.rs` (527 lines) into `src/{client,token,login}.rs`. Generated `native/index.{js,d.ts}`: CI checks drift and local builds are banned, so add an `if: failure()` step uploading the generated files as an artifact and commit them from it (workflow edit belongs to this task only). Token string wrapped in `secrets::protect` immediately; no `derive(Debug)` on `#[napi(object)]`; errors never echo input; documented that the JS string cannot be wiped.

**T9-6 TS Node session x TCP and native WS.** `tls.connect({minVersion:'TLSv1.3'})`; WS native via `ws` with `['shamir-v1']` and no origin; exporter via `exportKeyingMaterial(32, 'EXPORTER-ShamirDB-AUTH-v1', Buffer.alloc(0))` (empty vs absent context is identical in TLS 1.3; prove equality with rustls in e2e); resume for 0x01 with `resume_proof` (Ed25519 via the existing `@noble/ed25519`); `changePassword` with the real exporter; defaults `rejectUnauthorized: true` plus `tls.ca` (no `identity_sig` pin check in TS, task #622). No new server paths or listeners. The e2e harness starts only the browser listener and parses the first `listener bound` line (`e2e-harness.ts:190-200, 312-314`): teach it several listeners.

## 4. Order, risks, tests

Order: D0 docs (any time); Rust R1 -> R2 -> R3, then T9-2 and T9-3 in parallel, T9-4 after T9-3, T9-5 after T9-2..T9-4; TS S0 -> S1 -> S2 -> S3 -> S4, then T9-6. The TS branch is independent of Rust.

Sizes: T9-1 large in total (R1-R3 medium; S0-S4 small/medium); T9-2 medium; T9-3 large; T9-4 medium; T9-5 medium; T9-6 medium to large.

Risks: token privacy (HTTPS without verification; `http://` off loopback needs an explicit flag like the server; Debug/errors/logs); listener limits in CI (1000 connections, 100 per IP; 8 token sockets per token, 32 per owner; pre-auth 256 and 8 per subnet; Argon2 semaphore - fast KDF and separate tokens in tests); layout (folders at 7, `client.ts` 993 lines); rust-guard visibility/test-discovery; napi size (`opt-level = "z"`) grows with tungstenite and hyper; WebPKI against a self-signed test certificate (`CaUsedAsEndEntity`); TS e2e outside the merge gate; `ClientError` is not `non_exhaustive` (crate is `publish = false`, alpha).

Tests: Rust `crates/shamir-client/tests/matrix/` (`main.rs`, `common.rs`, `session_ws.rs`, `resume.rs`, `token_http.rs`, `token_ws.rs`, `login.rs`): ping/execute/push per cell; ticket TCP -> WS allowed, 0x01 -> browser rejected; Origin in the browser profile; uniform 401; 403 on a wrong channel; `not_supported_over_rest`; 4011 on revoke, 4012 on a short TTL; login success and wrong password; forged `server_signature` and class mismatch as a unit test with an emulated server in `src/tests/wire/`. Integration tests run in the full CI tier only; the server does not change. TS: `__tests__/connection/session/{e2e-tcp-native,e2e-ws-native,e2e-change-password-ipc}.test.ts`, run through the nightly workflow dispatch.

## 5. Documentation (T9-7)

Canonical page: `docs/guide-docs/guide/client-matrix.md`, linked from `guide/README.md`; never duplicate the table, only link to it from: protocol spec README ("Architecture": server cells and why the three are forbidden), `guide/token-api.md` ("Browser and transport choices"), `KNOWN_LIMITATIONS.md` (cells with status "no", TS without pin #622, password change over IPC until S4, no QUIC/mTLS), `crates/shamir-client-ts/docs/README.md`, `TRANSPORT_SPEC.md` (stale: browser path only), `TOKENS.md`, new `crates/shamir-client/README.md` and `crates/shamir-client-node/README.md`.

Statuses: `yes (test: <path>)`, `planned T9-n`, `no` (gap, not planned), `not needed` (reason), `impossible` (physical reason), `outside the protocol` (link). A cell turns to `yes` only in the PR that carries the e2e proving it. Updates: T9-1 none except S4; T9-2 Rust rows 1b, 2, 3, 6b-6d; T9-3 Rust rows 7, 8; T9-4 Rust row 10; T9-5 the whole napi column; T9-6 TS Node rows 1, 2, 6a, 6b.

Draft table:

| # | Handshake x transport (listener) | Server | Rust | napi | TS Node | TS browser |
|---|---|---|---|---|---|---|
| 1 | session x TCP+TLS (`tcp/tls_exporter`, 0x01) | yes | yes (`connect`, `connect_host`) | yes (`connect`) | planned T9-6 | impossible: no TCP |
| 1b | session x TCP plain (loopback, 0x00) | yes | planned T9-2 | planned T9-5 | no | impossible |
| 2 | session x WS native (`ws/tls_exporter`, `/shamir/v1`, 0x01) | yes | planned T9-2 | planned T9-5 | planned T9-6 | impossible: no exporter |
| 3 | session x WS browser profile (`ws/tls_no_export`, `/shamir/v1/browser`, 0x02, Origin) | yes | planned T9-2 | planned T9-5 | yes (`connect`) | yes (`connect`) |
| 4 | session x IPC (`unix/plain`, 0x00) | yes | yes (`connect_local`) | planned T9-5 | yes (`connectLocal`; password change: S4) | impossible |
| 6a | resume x TCP+TLS | yes | yes (`resume`, `resume_secure`) | planned T9-5 | planned T9-6 | impossible |
| 6b | resume x WS native | yes | planned T9-2 | planned T9-5 | planned T9-6 | impossible |
| 6c | resume x WS browser profile | yes | planned T9-2 | planned T9-5 | yes (`resume`, no resume_proof) | no: not exported |
| 6d | resume x TCP plain | yes | planned T9-2 | planned T9-5 | no | impossible |
| 6e | resume x IPC | yes (`ipc_resume_e2e`) | no | no | no | impossible |
| 7 | token x REST (`http`) | yes | planned T9-3 | planned T9-5 | yes | yes |
| 8 | token x WS native (`/shamir/v1/ws`, Authorization) | yes | planned T9-3 | planned T9-5 | yes | impossible: no header on WebSocket |
| 9 | token x WS browser (`/shamir/v1/ws/browser`, first frame) | yes | not needed | not needed | not needed | yes |
| 10 | password login -> token (`/auth/begin`, `/finish`) | yes | planned T9-4 | planned T9-5 | yes (`login`) | yes (`login`) |
| - | token x TCP / IPC / `kind: ws` | outside the protocol | - | - | - | - |
| - | session x HTTP | outside the protocol | - | - | - | - |
| - | QUIC, mTLS | not in code | - | - | - | - |

## 6. Orchestrator decisions on the open questions

1. **HTTP client in Rust (T9-3).** Decided at the T9-3 brief, not now (it does not block the pre-refactor). Leaning: `hyper` 1.x with features `client` and `http1` (new crates `want`, `try-lock`, both MIT and allowed by `deny.toml`, must pass the 7-day cooldown) over a hand-written HTTP/1.1 on `httparse`. No version bump of any existing dependency.
2. **TS Node `connect()` default.** Non-breaking: keep the browser-profile default; add an explicit `transport` option in T9-6.
3. **Extra cells the protocol admits but T9 does not schedule** (resume x IPC for all clients, TCP plain for napi and TS Node, browser resume export, `identity_sig` pin in TS #622): collected as a follow-up task T9-8.
4. **Browser profiles for native clients.** Session x WS browser profile: yes (useful behind a TLS-terminating proxy that breaks the exporter). Token x WS browser route for native clients: no.
5. **napi generated files.** Artifact upload on drift in the `node-binding` job (workflow edit inside T9-5); no local `napi build`.

## T9-1b S0-S2 report

Status: DONE as written, UNVERIFIED by any compiler or test run (no local builds by policy); verification is CI only (`ci.yml` full tier and `ts-e2e-nightly.yml` via dispatch on the branch).

- **S0:** `connect.test.ts`, `resume.test.ts`, `e2e-connect-local.test.ts`, `e2e-resume.test.ts` were copied to `crates/shamir-client-ts/src/__tests__/connection/session/` with only relative import paths fixed (`../../index.js` to `../../../index.js`, `../../core/...` to `../../../core/...`, `../e2e-harness.js` to `../../e2e-harness.js`, REPO_ROOT depth one level deeper in `connect.test.ts`). The four old copies were removed by the orchestrator (pure renames); `__tests__/connection` has 4 entries.
- **S1:** new `core/protocol/session/{binding.ts,endpoint.ts,index.ts}` (`ChannelBinding`, `browserWsBinding()`, `ipcBinding()`, `SessionEndpoint`, `SessionLink`). `runHandshake` takes `ChannelBinding | number` (legacy numeric `transportKind, bindingMode` form kept, so `core/protocol/__tests__/protocol.test.ts` is unchanged); the exporter is NOT added to the SCRAM auth_message (wire identical). `ShamirClient` stores the binding in the private field `_channelBinding` (no new public member).
- **S2:** bodies of `connect`, `connectLocal`, `resume` moved to `core/protocol/session/{open.ts,resume.ts}` (`openSession`, `openScramSession`, `openLocalSession`, `resumeSession`) returning the plain `SessionOpened` (`opened.ts`); the statics in `client.ts` are thin delegates through a private `fromOpened`. `client.ts` went from 993 to 777 lines. Import graph: `client.ts` -> `protocol/session/index.ts` -> `{open,resume}.ts` -> `{protocol.ts, framing.ts, client/client-internals.ts, types/connection.ts, binding.ts}`; no session file imports `client.ts`, so there is no cycle.
- **Tests added:** `core/protocol/session/__tests__/binding.test.ts` (profile constants, fresh exporter arrays, `auth_init.binding_mode` for the browser, ipc and legacy numeric forms; fake socket, no network).
- **Not touched:** `src/index.ts`, `src/browser.ts`, `core/protocol/scram.ts`, token path, `package.json`, lockfiles, workflows. `changePassword` still hard-codes the browser binding (S4, out of scope).
- **Layout:** `python -I scripts/ci/check_layout.py` reports OK.
- **Known caveats:** `_channelBinding` is stored but not yet read (S4 will use it). `resume` yields `normalizedUser: undefined` exactly as before.

## T9-1 R2 report (Rust client session + mux move, behaviour unchanged)

Status: DONE in code. No local compile/test was run (policy); verified by a compiler-style read plus `cargo fmt -p shamir-client -- --check` and `python -I scripts/ci/check_layout.py`.

What moved (bodies verbatim, only imports and visibility adjusted):
- `client/connect.rs` -> `client/session/client.rs` (`SessionMeta`, `struct Client`, `Client::assemble`, the `cfg(test)` accessors) and `client/session/entry.rs` (`connect`, `connect_until`, `connect_host`, `handshake`, `connect_local`, `resume`, `resume_secure`, `verify_auth_ok`, `resume_wire_exchange`, `verify_resume_identity_proof`, `ResumedHandshake`, the `#[path]` link to `interner_cache/resume/security.rs`, now `../../interner_cache/resume/security.rs`).
- `client/options.rs` -> `client/session/options.rs` (byte-identical).
- From `client/transport.rs` -> `client/session/mux/lifecycle.rs` (`PendingSender`, `ClosedState`, `PendingRegistry`, `PendingMap`, `Lifecycle`, `ShutdownGuard`, `LifecycleHooks`), `mux/reader.rs` (`DemuxResult`, `IncomingEnvelope`, `decode_frame`, `await_pending_response`, `reader_task`), `mux/hooks.rs` (`HookPoint`, test-only).
- `client/transport.rs` keeps `WriteSink`, `connect_tcp`, `connect_ipc`, `connect_with_timeout` and all `Wire*` structs, and re-exports `HookPoint` (cfg(test)), `ClosedState`, `Lifecycle`, `LifecycleHooks`, `ShutdownGuard`, `await_pending_response`, `reader_task`, so every `crate::client::transport::{..}` path still resolves. `client/mod.rs` keeps every public and `pub(crate)` path.

Visibility widenings (all within the crate, none public): the `Client` fields were `pub(super)` (scope `client`) and are now `pub(in crate::client)` (same scope, path spelled out because the struct moved one level down); `resume_security` was `pub(super)` and is now `pub(in crate::client)`; `session::entry` is `pub(in crate::client)`; `session::mux` is `pub(crate)`. The effective visibility scope is unchanged.

Dropped: the `pub(super) use resume_security::deserialize_resume_ticket;` re-export (the `WireResumeOk` serde attribute now names `crate::client::session::entry::resume_security::deserialize_resume_ticket` directly).

Public API untouched: `lib.rs` re-exports, options file byte-identical. Guard tests untouched.

DONE by the orchestrator: `connect.rs` and `options.rs` were removed (renames into `session/`); `client/` has 6 entries.

Not verified locally: compilation, clippy, tests (CI only).

## T9-1 R1 report (Rust client handshake module, one SCRAM for every transport)

Status: DONE in code. No local compile/test was run (policy); checked by a compiler-style read plus `cargo fmt -p shamir-client -- --check` and `python -I scripts/ci/check_layout.py`. Verification is CI only.

What moved (bodies verbatim, only imports and visibility adjusted):
- `client/transport.rs` -> `client/handshake/wire.rs`: all eight `Wire*` structs. `transport.rs` re-exports them under `#[cfg(test)]` (non-test code no longer uses that path), so `crate::client::transport::Wire*` still resolves for the guard tests.
- `client/session/entry.rs` -> `client/handshake/auth_ok.rs` (`VerifiedAuthOk`, `verify_auth_ok`) and `client/handshake/resume.rs` (`ResumedHandshake`, `resume_wire_exchange`, `verify_resume_identity_proof`).
- New `client/handshake/scram.rs`: `run_scram<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(r, w, ScramRequest) -> ScramOutcome`, the single SCRAM exchange. `ScramRequest` carries username, `TransportKind`, `BindingMode`, exporter `[u8; 32]`, trusted pin, accept-new-host, password slice and `phase: Option<&AtomicU8>`. `Client::handshake` (TCP+TLS) and `Client::connect_local` (IPC) now both do: dial (unchanged) -> `run_scram` -> `Client::assemble` with `session_meta(outcome, request_timeout)`.
- `client/` has 7 entries: `api.rs cursor_stream.rs handshake/ mod.rs session/ subscription/ transport.rs`; `handshake/` has 5.
- New guard test `src/tests/wire/scram_binding_tests.rs` (`run_scram` over `tokio::io::duplex` against an emulated `ServerHandshake`; bindings `(Tcp, TlsExporter, [0x5A; 32])` and `(Unix, None, zeros)`; asserts the `auth_init.binding_mode` byte, completion, session id and `server_query_version`; not `cfg(unix)`).

Differences found between the two old SCRAM copies (each preserved as a parameter):
1. Phase tracking: only the TCP copy stored `PHASE_SCRAM` (start), `PHASE_KDF` (before `spawn_blocking`) and `PHASE_SCRAM` (after the join); IPC stored none -> `ScramRequest::phase` is `Some` for TCP, `None` for IPC (same three store points).
2. Channel binding: TCP `(Tcp, TlsExporter)` + `.tls_exporter(exporter)`; IPC `(Unix, None)` with the builder default (all zeros) -> now explicit parameters, IPC passes `[0u8; 32]` (identical to the default).
3. Order: TCP built the `HandshakeBuilder` before `split(tls)`; now `split` happens first (no I/O in between, `PHASE_SCRAM` is still stored before any frame I/O).
4. Writer variant: `WriteSink::Tls(w)` vs `WriteSink::Ipc(w)` stay in the call sites. `connect_local` still has no `handshake_timeout`. Error strings, `MAX_PRE_AUTH_FRAME`, Argon2 in `spawn_blocking`, `Zeroizing` of password and ticket unchanged. The two `SessionMeta` constructions were identical and are now `session_meta()` in `entry.rs`. Everything else (comments aside) was identical.

Visibility widenings vs base (all within the crate, none public): `VerifiedAuthOk` and its fields private -> `pub(crate)`; `verify_auth_ok` private -> `pub(super)` (inside `handshake`); `PHASE_SCRAM`/`PHASE_KDF` private (in `entry.rs`) -> `pub(in crate::client)` (in `scram.rs`); new `pub(crate)` items `ScramRequest`, `ScramOutcome`, `run_scram`; `resume_wire_exchange`/`ResumedHandshake`/`verify_resume_identity_proof` keep `pub(crate)`; `handshake::wire` is `pub(in crate::client)`. `PHASE_DNS/TCP/TLS` and `phase_name` stay in `entry.rs`.

Public API untouched (`lib.rs`, option structs, `Client::connect/connect_host/connect_local/resume/resume_secure`). No Cargo.toml/lockfile change. Only the doc links of `Client::resume` changed (full paths to the moved `Wire*` types) and the `connect_local` doc sentence now says it shares `run_scram`.

BLOCKED: nothing. Not verified locally: compilation, clippy, tests (CI only).
