# T9-3 / T9-4 - token REST, token WebSocket and password login in the Rust client (detailed design)

Read-only design by the `oh` reviewer against the code of master `4c150fbbc` plus the T9-2 branch. Base for implementation: the merged T9-2 (which contains R3). Server paths are given for master.

## 0. Finding that fixes the order

tungstenite 0.30 logs the whole upgrade request, including the `Authorization` header, at `trace` level (`handshake/client.rs:197`, the line is a ready byte string, so `HeaderValue::set_sensitive` does not hide it). Therefore the token WebSocket upgrade MUST NOT go through `client_async*`; it is done by hyper (`with_upgrades`) and then `WebSocketStream::from_raw_socket`. Order: REST on hyper first (T9-3a), then the token WebSocket (T9-3b), then password login (T9-4).

## 1. How `Client` works today and what the token WebSocket changes

Session path: `client/api.rs` `roundtrip` - `rid` from an `AtomicU32` (start 1), body encoded with `write_named` into a zeroizing buffer, envelope `{sid, rid, req: bin}` written with a u32 prefix, a oneshot registered before the write, answers delivered by `reader_task` (`{rid,res}`, `{rid,error}` -> `ClientError::Server`, push -> subscription router). The session HMAC key (`derive_session_hmac_key(session_id)`) is NOT on the envelope; it tags only the body of admin requests (`create/list/revoke_api_token`, `revoke_user_tokens`, `create_scram_user`, `set_replicator`). `server_query_version >= 2` makes `execute` announce interner epochs.

Token WebSocket on the server (`crates/shamir-server/src/sync/connection/token_ws/`): one BINARY message = one msgpack map with NO u32 prefix (`framer.rs`); same envelope `{sid,rid,req}`; no HMAC; wrong sid closes 4010; revoke 4011; expiry 4012; rate limit `{rid,error:"rate_limited"}`; the first server message after 101 is `{ok:true, sid:bin32, expires_at_ns}`. Admin requests are cut by the scoped handler (`token_request_denied`).

One `Client`, parameterised:
1. `WsByteStream` gets `WsFraming::{Prefixed, Raw}`. `Raw`: on write buffer to a complete `[len][payload]` and send `payload` (drop the first 4 bytes) as one message; on read put `len_be` in front of each message body; empty or oversized message -> `InvalidData`; `Close(Some(f))` -> one `io::Error` carrying the close code and reason, then EOF.
2. `reader_task` maps that error to `CloseCause::PeerClosed { code, reason }` (new variant; `CloseCause` is `non_exhaustive`).
3. `WriteSink::TokenWs(WriteHalf<WsByteStream<TokioIo<Upgraded>>>)`.
4. `SessionMeta` and `Client` get an `auth: AuthKind` (`#[non_exhaustive] enum AuthKind { Scram, Token }`, getter `Client::auth_kind()`); token sessions have `session_id = sid`, `pinned_hash = [0; 32]`, no resumption ticket, `server_query_version = 0`.
5. Local refusal on token sessions BEFORE any HMAC or body is built (a password of `create_scram_user` must never reach the network): `create_api_token`, `list_api_tokens`, `revoke_api_token`, `revoke_user_tokens`, `create_scram_user`, `set_replicator`, `repl` -> `ClientError::Db { code: "token_request_denied", message: "requires a SCRAM session" }` through a helper `require_scram`.
A flag is enough (the data plane is byte-identical); a wrapper type can be added later.

## 2. REST client (`TokenHttpClient`)

API: `connect(TokenHttpOptions)` (dial + TLS up front), `ping`, `request(&DbRequest) -> DbResponse`, `execute(..)`, `wait_for_visibility`, `get_ddl_op_status`, `close`. Local deny-by-default filter in `request`: allowed `Ping`, `Execute`, `WaitForVisibility`, `GetDdlOpStatus`; `Tx*` and cursors -> `Db { code: "not_supported_over_rest" }`; everything else -> `Db { code: "token_request_denied" }`.

Request: `POST /shamir/v1/request`, headers exactly `Host`, `Authorization: Bearer shk1...` (sensitive), `Content-Type: application/msgpack`, `Accept: application/msgpack`, `Content-Length`; never `Cookie`, `Origin`, `Expect`, `Connection`, `Transfer-Encoding`, `Accept-Encoding`; body `write_named(DbRequest)`. `ping` is `GET /shamir/v1/ping` without a body.

Response (the server only sends full bodies with `Content-Length`): 200 requires `Content-Type: application/msgpack`, body <= 16 MiB, no `Content-Encoding`, decoded as `DbResponse` and mapped like the session path; 400/403 with a decodable `DbResponse::Error` -> `ClientError::Db`; 401 -> `Http { status: 401, code: Some("unauthorized") }`; 404/405/408/413/415/431, 3xx and bodiless 400/403 -> `Http { status, code: None }`; 429/503 -> `Http { .., retry_after }` from `Retry-After` (delta-seconds only, clamped 1..=3600). New `ClientError` variants: `Http { status: u16, code: Option<String>, retry_after: Option<Duration> }` (`code` only if it matches `[a-z][a-z0-9_]{0,63}`; `Display` never contains a body or headers) and `InvalidToken` (no payload).

Keep-alive: lock-free pool (`scc::Bag` plus an `AtomicUsize` bound of 4). A request is retried only if hyper returned it unsent (`TrySendError::take_message`) or the connection was found closed before sending; never after possible writes (a POST `Execute` is not idempotent).

TLS: rustls 1.3 only, aws-lc-rs provider, `with_root_certificates(roots)`, no client auth, ALPN `http/1.1` (reject any other negotiated protocol), early data off. The standard WebPKI verifier checks chain and host name; `make_client_config_app_authenticated` (Ed25519-pin, no chain check) is forbidden on the token path. Trust: `#[non_exhaustive] enum TokenTrust { WebPkiRoots, CustomRoots(..) }` with constructors `pem(&[u8])` / `der(..)` (no rustls types in the public API). `webpki-roots` 1.0.10 is already in the lockfile. Tests trust `<data_dir>/cert.pem` (SAN `localhost`, rcgen `NoCa`).

Endpoint: `TokenEndpoint { addr, server_name, scheme }` instead of a URL (no path, query, userinfo or redirect can be injected): `TokenEndpoint::https(addr, server_name, trust)` and `TokenEndpoint::plain_loopback(addr)` (refuses a non-loopback `addr.ip().to_canonical()` before opening a socket; remote plaintext bearer is not supported in v1). No redirects (3xx -> `Http`), no cookies.

## 3. HTTP stack decision

Chosen: `hyper` 1.x with features `client` + `http1`. It adds only two new packages to the lockfile: `want` 0.3.2 and `try-lock` 0.2.5 (both MIT, older than the 7-day cooldown, licences allowed by `deny.toml`). `hyper-util` only with feature `tokio` (not `client`). `http` comes through `hyper::http`. Dependency diff for `crates/shamir-client/Cargo.toml`:

```toml
hyper          = { version = "1", default-features = false, features = ["client", "http1"] }
hyper-util     = { version = "0.1", default-features = false, features = ["tokio"] }
http-body-util = "0.1"
bytes          = "1.9"
webpki-roots   = "1"
```

Lockfile without a build: `cargo metadata --format-version 1 --offline > NUL` rewrites `Cargo.lock`; the expected diff is `want`, `try-lock`, `"want"` in hyper's dependencies and the five edges in `shamir-client`; anything else is rolled back. A manual HTTP/1.1 client on `httparse` was rejected (500-700 lines of security-sensitive parser, chunked handling, keep-alive state).

## 4. Token as a secret

Public paths: `shamir_connect::server::token::{parse_token, TokenId}` (the `auth` module itself is private). `pub struct ApiToken(Zeroizing<String>)` in `session/token/api_token.rs`: `new`, `TryFrom<&str>`, validation through `parse_token` (71 characters, `shk1.`, canonical base64url), failure -> `ClientError::InvalidToken` without the input; `id()` (non-secret), `reveal()`, `TryFrom<&CreatedApiToken>`; `Debug` prints `ApiToken { id, secret: "<redacted>" }`; no `Display`. The client holds a pre-built `HeaderValue` with `set_sensitive(true)` over a zeroizing `Bytes` owner. hyper without its `tracing` feature logs nothing; own errors never format the request or headers. Unwipeable copies (hyper write buffer, rustls plaintext buffers, the login response body before parsing) are documented.

## 5. Token WebSocket (T9-3b)

`Client::connect_token_ws(TokenWsOptions)` in `session/entry_ws.rs`: TCP + TLS (WebPKI, no Ed25519 pin) -> hyper http1 handshake with `with_upgrades` -> `GET /shamir/v1/ws` with `Host`, sensitive `Authorization`, `Connection: Upgrade`, `Upgrade: websocket`, `Sec-WebSocket-Version: 13`, `Sec-WebSocket-Key`, exactly one `Sec-WebSocket-Protocol: shamir-v1`, no `Origin`. Non-101 -> status table of section 2 (403 with `DbResponse::Error` -> `Db`). On 101 verify `Sec-WebSocket-Accept`, the subprotocol and the absence of `Sec-WebSocket-Extensions`, then `WebSocketStream::from_raw_socket` (`max_message_size = max_frame_size = MAX_FRAME_SIZE_DEFAULT`), read the binding message within `handshake_timeout` (exactly three keys, `ok == true`, 32-byte non-zero `sid`, `expires_at_ns > 0`), split and `Client::assemble` with the `TokenWs` sink. Close codes -> `CloseCause::PeerClosed { code, reason }` (reason <= 123 bytes, printable): 4003 no pong, 4010 sid mismatch, 4011 revoked, 4012 expired, 1013 busy, 1000 normal; Raw mode only.

## 6. Password login (T9-4)

Available to the client: `shamir_connect::server::http_login::{build_transcript, encode_scope, build_server_signature, ...}`, `common::scram::DerivedKeys`, `common::crypto::{hmac_sha256, constant_time_eq, try_random_array}`, `NormalizedUsername`, `server::token::parse_token`. API: `login(endpoint, LoginOptions) -> LoginOutcome { token: ApiToken, .. }` with `LoginScope` (default label `http-login`, read-only, REST). Order: normalise the user, sort/dedup databases and `encode_scope`; 32-byte non-zero nonce; the expected security class comes from the endpoint, never from the challenge (https 0x01, https behind a proxy 0x02, loopback http 0x00); `POST /shamir/v1/auth/begin` on a fresh connection; strict challenge parsing (`deny_unknown_fields`, `argon2id`, version 1, binding mode 3), check class, optional `listener_id` pin and optional expected KDF; `build_transcript` validates the KDF limits BEFORE Argon2; Argon2 in `spawn_blocking`; proof = `client_key XOR HMAC(stored_key, T)`; `POST /shamir/v1/auth/finish`; verify `constant_time_eq(server_signature, build_server_signature(..))` FIRST (mismatch: wipe the token, `Handshake("login: server signature mismatch")`), then token id == returned id, scope fields equal to the request, `expires_at_ns > created_at_ns`. Server errors -> `Http { status, code, retry_after }`. No Ed25519 pin: TLS authenticates the server.

## 7. PR split, layout and tests

Layout (limit 7): `transport/` 6 -> 7 (`token/`), `session/` 6 -> 7 (`token/`), `handshake/` 5 -> 6 (`bearer.rs`) -> 7 (`login/`), `tests/matrix/` 6 -> 7 (`token/`); the 7-entry `src/` and `src/tests/` are not touched.

- **T9-3a - REST and dependencies:** the diff of section 3, `transport/token/{mod,endpoint,tls,http_conn,status}.rs` (+ `tests/`), `session/token/{mod,api_token,options,http_client}.rs` (+ `tests/`), `ClientError::{Http, InvalidToken}`, re-exports in `lib.rs`. Unit tests on a local fake `TcpListener` (exact headers, keep-alive reuse, `Connection: close`, chunked response, 307 not followed, body limit, status table, plaintext refusal) and `ApiToken` tests; integration `tests/matrix/token/{mod,common,http}.rs` (ping, execute, 401, 403 `token_read_only`, `not_supported_over_rest`, loopback `http://`).
- **T9-3b - token WebSocket:** `transport/token/dial_ws.rs`, Raw framing in `ws_stream.rs`, `WriteSink::TokenWs`, `handshake/bearer.rs`, `CloseCause::PeerClosed`, `AuthKind` and local refusals, `connect_token_ws`. Tests: Raw stream, binding decode, refusals write nothing to the duplex; integration ping/execute/push/cursor, 401, 403 `token_channel_denied`, 4011 after revoke, 4012 with a 2 s TTL.
- **T9-4 - login:** `handshake/login/{mod,wire,flow,verify}.rs`; tests on an emulated server (forged signature, class/pin mismatch, KDF out of bounds, id mismatch) and integration (login, then ping; wrong password `Http{401,"authentication_failed"}`; read-write `login_write_forbidden`; superuser `login_account_denied`).

Test server: own `token/common.rs` (the T9-2 `make_config` stays private): `ListenerKind::Http` on `127.0.0.1:0` with `tls: Some(true)` (self-signed on loopback is allowed) and one with `tls: Some(false)`, `login.enabled = true` where needed; seed the owner like `crates/shamir-server/tests/e2e/http_rest/common.rs::seed_owner`; tokens via `handle.token_service.create(NewToken{..})` (as in `http_ws/session_loop.rs`) and through the public path (`create_scram_user`, then the user's `create_api_token`). Login limits in CI: 10 credits per minute per /24, so at most 4 logins per server; the wrong-password test uses a separate user.

Risks and open points: `server_query_version` of token sessions is 0 (the binding frame carries none; the server does not change in T9); `ClientError` is not `non_exhaustive` (alpha, `publish = false`); remote plaintext bearer unsupported; default roots are the webpki bundle only (OS store would need `rustls-native-certs`); napi binary size grows with hyper; each PR updates only the matrix cells its e2e proves (rows 7, 8, 10).
