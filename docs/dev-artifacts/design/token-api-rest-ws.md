# Token API: REST and WebSocket access by bearer token

Status: DESIGN, revision 3 (2026-11-01). Requested by the owner on 2026-11-01. Nothing in this document is implemented. Revision 3 adds the owner follow-up of 2026-11-01 (users must be able to be issued tokens for HTTP and WebSocket: per-channel tokens and self-service issuance, sections 6.2, 7). Revision 2 folded in the independent review (24 findings, three of them HIGH; each is spot-checked against the code and marked "review Fn" where it changed the text). The work is split into tasks listed in section 16.

## 0. Request and interpretation

Owner request: expose a REST API to the outside, authenticated by a token in the request headers, over four channels:

| # | Channel | Meaning |
|---|---|---|
| 1 | HTTP + Bearer | plain HTTP, `Authorization: Bearer <token>` |
| 2 | WebSocket | plain WebSocket, the token in the upgrade request headers |
| 3 | channel 1 + certificate | HTTPS (TLS server certificate) |
| 4 | channel 2 + certificate | WSS (TLS server certificate) |

Interpretation used here (to be confirmed, section 17): "certificate" means the TLS server certificate that `https` and `wss` need. Client certificates (mutual TLS) are not part of this design.

## 1. Goals and acceptance criteria

1. A third-party program that has only a token string can call the database over HTTP/1.1 (`POST`, msgpack body) and over a WebSocket, on all four channels, with no SCRAM handshake and no database client library.
2. Every request runs as a real database principal: the existing authorization (owner/group/mode access, `Actor`, read-only node gate, query limits, byte budget, rate limits) applies unchanged. A token can never do more than its owner could do over a SCRAM session, and by default does less (section 6.4).
3. Tokens are issued, listed and revoked by an administrator; revocation is effective for the next request, cancels in-flight token requests that have not yet been authorized, and closes live token WebSockets (section 9.4).
4. A plaintext channel cannot be opened by accident: it is loopback-only unless the operator sets explicit opt-ins (section 10.2).
5. A wrong, expired, revoked or malformed token yields one uniform `401` without timing or body differences, and never appears in logs, metrics labels, audit records or error text.
6. A token session cannot be reached, reused or escalated through any other listener, and no SCRAM session can be reached through a token listener (section 4, rule 2).
7. Observable proof: an end-to-end matrix of 4 channels x {valid, wrong, expired, revoked, read-only violation, scope violation, oversized body, missing header, ...} (section 15) is green in CI on Linux and Windows.

## 2. Non-goals (this design)

- Mutual TLS, OAuth/OIDC, JWT, cookies, CSRF protection (Bearer in a header has no ambient authority), CORS (no `Access-Control-*` header is ever emitted in v1; a cross-origin browser client is out of scope).
- JSON request bodies (deferred, section 17.3), a resource-style REST surface, OpenAPI generation.
- Browser WebSocket clients (browsers cannot set `Authorization` on a WebSocket).
- Admin operations over a token: user admin, superuser/replicator flags, password change, replication, destructive-operation HMAC confirmation (section 6.4). The admin REST sketch of `ADMIN_UI_HOSTING.md` (`/shamir/v1/admin/<command>`, admin session as Bearer) stays an unimplemented specification and is not served by this feature.
- Interactive transactions, cursors and subscriptions over REST (they need a server-side session); they are available on the WebSocket channels.
- Token replication between nodes: the token store is node-local in v1; creating and revoking tokens works on the node that owns the data directory (on a read-only replica through the offline CLI only), see section 7.
- Per-token quotas beyond the rate limit; per-request usage statistics persisted per token.

## 3. What exists today (verified in the tree at `cba8515e`)

- Transports: TCP+TLS (`shamir-transport-tcp`), WebSocket over TLS (`shamir-transport-ws`, `/shamir/v1` native, `/shamir/v1/browser`), local IPC. All authenticate with SCRAM-Argon2id plus channel binding, then run the same post-handshake loop (`shamir-server/src/sync/connection/request_loop.rs`, the `Framer` abstraction in `sync/framer.rs`, `RequestHandler::handle(&Session, req_bytes, &ConnectionServices)` implemented by `ShamirDbHandler`).
- **The request envelope names its session**: every post-auth frame is `{sid, rid, req}` (`shamir-connect/src/common/messages/envelope.rs`, `RequestEnvelopeView`); `dispatch_request_view` resolves `sid` in the server-wide `SessionStore` (`server/infra/dispatch.rs`), not from the connection. `Session` carries `permissions`, `transport_kind`, `binding_mode`, `channel_binding_at_auth`, `post_auth_bucket`, and has no scope or origin field (`server/state/session.rs`).
- **Privilege checks read `session.permissions`, not the `Actor`**: the coarse admin gate, `create_scram_user`, `set_superuser`, `set_replicator`, the replication handler (`is_superuser || is_replicator`), and the transaction handlers all test `session.permissions` (`engine/db_handler/handler.rs`, `handlers/admin.rs`, `handlers/repl_handler.rs`, `repo_admin/tx_handlers.rs`). `session_actor` maps `is_superuser` to `Actor::Admin`. `ChangePasswordChallenge/Verify` are not superuser-gated.
- Plain WebSocket does not exist (`TRANSPORT_WS.md`: "plain WS removed in v1"); plain TCP exists as `ProfileKind::Plain`, refused on a non-loopback address by `Config::validate`.
- Listener configuration: `ListenerConfig { kind: tcp|ws|unix, addr, profile (required), path, kdf_override, browser_origin_allowlist }`; TLS `TlsConfig { cert_path, key_path }`, self-signed on first start when both files are absent (`security/tls.rs`, which justifies self-signed by the application-level Ed25519 server pin that a token client does not have).
- An HTTP/1.1 server exists for observability (`axum` 0.7.9, `http1`+`tokio`): default `127.0.0.1:9090`, non-loopback needs `allow_public_metrics`. The lockfile has hyper 1.9.0, http 1.5.0, tungstenite/tokio-tungstenite 0.30.0 (one `http` 1.x line); axum's `ws` feature would pull a second tokio-tungstenite and must stay off.
- Close codes: `4003` is `heartbeat_timeout` (`framer.rs`, `TRANSPORT_WS.md` section 6), `4000-4002` are auth/version/busy. `SessionStore::admit` caps sessions per user (16) and `SESSION_MAX_AGE` is 24 h.
- Response byte budget (RI-15) is held by a guard that `run_with_handoff` keeps alive until the frame is written (`byte_budget.rs`).
- `BatchTreeVisitor` + `BatchOp::is_write()` (exhaustive) implement the read-only replica gate; `BatchOp::Subscribe` lives inside `Execute`.
- `WsFramer<S>` is generic over any `AsyncRead + AsyncWrite`, so it also fits a plain socket.

## 4. Architecture

```
          channel 1/3                          channel 2/4
 curl/any HTTP client --POST--+     +--GET Upgrade-- any WS client
                              |     |
                    +---------v-----v---------+  plaintext (1,2) or TLS 1.3 (3,4)
                    |  token listener (hyper) |  one port, two endpoints
                    +-----------+-------------+
                                | head only: Authorization: Bearer <token>
                        +-------v--------+
                        | TokenVerifier  |  id lookup, constant-time MAC compare,
                        | + TokenStore   |  expiry/revoked/owner state (all in memory)
                        +-------+--------+
        REST: per-request token session | WS: token session for the socket lifetime
                                |      (TokenSessionStore, NOT the SCRAM SessionStore)
                        +-------v--------+
                        | ScopedHandler  |  deny-by-default allowlist + scope
                        +-------+--------+
                                |
                     RequestHandler::handle(...)   (ShamirDbHandler)
```

Rules (review F1, F2, F3, F8, F9):

1. The token path never reaches SCRAM, Argon2, tickets or `changepw`.
2. **Separate session domains.** Token sessions live in their own `TokenSessionStore` with its own accounting and caps; the SCRAM `SessionStore` never contains a token session and a SCRAM listener never consults the token store. A token WebSocket connection is bound to exactly one token session: its request loop **rejects any frame whose `sid` differs from the connection's session id** and the session id is a fresh random 32-byte value (never zeros). This closes the cross-listener replay in which a low-privilege SCRAM user sends a stolen token `sid` over their own connection (and the reverse).
3. `Session` gets a server-only field `origin: SessionOrigin` (`Scram` | `Token { token_id, scope, cancel }`); `ScopedHandler` and the handlers read the scope from the `Session` they receive, never from per-connection state. **No wire enum changes**: `TransportKind` and `BindingMode` are embedded in `auth_message` and are not extended (adding a value is an AUTH minor bump, and `BindingMode::None` is documented as loopback-only plaintext); audit and metrics map `origin` to their own strings (`repo_audit.rs`, `handshake.rs` mappings are extended).
4. Token permissions are constructed, not copied: `SessionPermissions::new(is_superuser = false, is_replicator = false, roles = owner's roles)` regardless of the owner's flags. Every privileged gate keeps reading `session.permissions`, so a superuser- or replicator-owned token is an ordinary user to the handler.
5. `ScopedHandler` is a wrapper around `ShamirDbHandler`, deny-by-default (section 6.4); `ShamirDbHandler` itself gets no authentication code. The token request loop is a variant of `run_request_loop` (section 9), not an unchanged reuse.
6. One listener kind serves both endpoints; the four channels are the product of the switches `tls` and `endpoints`.

## 5. Crate and module layout

- New crate `shamir-transport-http` (family of `shamir-transport-tcp/ws/ipc`): the HTTP/1.1 connection driver on **hyper 1.x** (no hand-written head parser), TLS accept (reusing `shamir-transport-tcp::tls`), `Authorization` extraction, and the WebSocket upgrade through `hyper::upgrade` followed by `tokio_tungstenite::WebSocketStream::from_raw_socket` with the 0.30 version already in the tree (review F13: `accept_hdr_async` reads its own request and cannot share a port with `POST`). No dependency on `shamir-db`. Do not enable axum's `ws` feature. Any new dependency must pass `cargo deny` and the 30-day cooldown; prefer `hyper`, `hyper-util`, `http`, `http-body-util` if they are already locked, and `hmac`, `sha2`, `subtle` which `shamir-connect` already depends on.
- `shamir-connect` (server side): token model, `ApiTokenStore` trait, HMAC verification (pure, no I/O), `TokenSessionStore`, `SessionOrigin`.
- `shamir-server`: the fjall store (`api_tokens_v1` keyspace next to the user directory), `ListenerKind::Http` wiring, `ScopedHandler`, the token request loop, CLI subcommands, metrics and audit hooks.
- `shamir-query-types`, `shamir-query-builder`, `shamir-client`, `shamir-client-ts`: the online token administration operations (task T5).
- **Layout prerequisite (review F14).** These folders are at the 7-entry limit and these files are near the 1000-line limit, so wiring needs a split first (task T0b): folders `shamir-server/src`, `shamir-server/src/security`, `shamir-server/src/sync/connection`, `shamir-server/src/sync/connection/session`, `shamir-server/src/boot`, `shamir-connect/src/server`; files `server_launcher/launch.rs` (999 lines), `boot/config.rs` (944), `db_handler/handler.rs` (947). The `CLAUDE.md` crate list is updated in T3 only.

## 6. Tokens

### 6.1 Format

`shk1.<id>.<secret>`: `id` is 16 random bytes (22 base64url characters), `secret` is 32 random bytes (43 base64url characters), both from the OS CSPRNG, no padding; the separator is `.` (outside the base64url alphabet; `.` is legal in an RFC 6750 `b64token`), and parsing is positional with fixed lengths (review F15), total 71 characters. The prefix makes leaked tokens greppable by secret scanners and versions the format. The secret is shown exactly once, at creation.

### 6.2 Storage

Persisted record (msgpack) keyed by `id` in a new fjall keyspace `api_tokens_v1`, with a secondary index `owner_user_id -> [id]`:

```
id: [u8;16]
owner_user_id: [u8;16]          // fresh per account: a re-created account does not inherit tokens
label: String                   // unique per owner, not secret
mac: [u8;32]                    // HMAC-SHA256(token_key, "shamir-api-token" || format_version || key_id || id || secret)
key_id: u8                      // 1 in v1
created_at_ns, expires_at_ns    // expiry mandatory, capped by tokens.max_ttl_days
mode: ReadOnly | ReadWrite
channels: Rest | Ws | Both       // which endpoint the token may be used on; default Both
databases: Option<Vec<String>>  // allowlist; None = every database the owner may access
revoked_at_ns: Option<u64>
format_version: u8
```

The secret is high-entropy, so a fast keyed MAC is correct (Argon2 would be a CPU lever against the server). `token_key` is 32 random bytes in `server_meta`, created at bootstrap. **It is never rotated in v1** (the `lockout_secret` precedent; `server_meta` keeps only one previous ticket key, which cannot serve tokens that live up to a year, review F10); `key_id` and the MAC input already contain the key identity so a later multi-key scheme is a pure addition. A leak of `token_key` is handled by revoking all tokens (`token revoke-all`) and re-issuing. A snapshot of the user store alone does not allow offline verification of guesses.

### 6.3 Verification (hot path, lock-free)

An in-memory `scc::HashMap<[u8;16], Arc<TokenState>, THasher>` mirrors the persisted records (loaded at boot, updated on create/revoke/owner change) together with a small `OwnerState { user_id, tokens_invalid_before_ns, present }` map fed by the directory's mutation events, so no request reads fjall (review F12). Per request, from the request head only:

1. Exactly one `Authorization` header, scheme `Bearer` (case-insensitive); the token is never read from the query string, a cookie or a body.
2. Fixed-length positional parse; malformed input gets the same `401` as a wrong token.
3. Look up `id`. When it is unknown, still compute an HMAC against a dummy so unknown-id and wrong-secret take comparable time.
4. Constant-time compare (`subtle`).
5. Validity: not revoked; not expired; the owner is present in `OwnerState` with the same `user_id`; `token.created_at_ns >= owner.tokens_invalid_before_ns` (kicking a user, changing roles and **backup restore** bump it for every user, so a restored backup cannot resurrect revoked tokens, review F11).
6. The token's `channels` must include the endpoint it is presented on (`rest` or `ws`); otherwise `403` with `token_channel_denied` (REST), or `403` before `101` (WebSocket upgrade). A `Rest`-only token cannot open a socket and a `Ws`-only token cannot call the REST endpoint.
7. Build the token session (section 6.4).

Failures feed a **separate per-subnet token-failure limiter** (an instance of the `RateLimiter` trait, never the SCRAM `LockoutStore`): feeding the owner's username into the SCRAM lockout would let anyone who learns a token id lock the owner out of SCRAM login (review F12). Success never resets another subnet's counters. The ordering of publish-then-close for revocation follows the lifecycle pattern already used in `shamir-client` (`ClosedState`): publish the revocation first, then cancel sessions.

### 6.4 Principal, scope and the deny-by-default allowlist

- A token acts as its owner: `Actor::User(principal64(owner))`, `SessionPermissions::new(false, false, roles)` (rule 4 above). Everything gated on `session.permissions` (create user, set superuser/replicator, replication, admin leaves, the destructive-operation HMAC check whose key derives from the session id) is therefore unreachable; the session id is never exposed in a way that lets a client "fix" that. `ChangePasswordChallenge/Verify` are not superuser-gated in the handler and are therefore **explicitly denied** by `ScopedHandler`.
- `ScopedHandler` is an allowlist on both channels. Allowed `DbRequest` variants: `Ping`, `Execute`, `WaitForVisibility`, `GetDdlOpStatus`, and on WebSocket additionally `TxBegin`, `TxExecute`, `TxCommit`, `TxRollback`, `CreateCursor`, `FetchNext`, `CancelCursor`. Denied: `CreateScramUser`, `SetSuperuser`, `SetReplicator`, `ChangePassword*`, `Repl`, and any variant added later until it is classified. Inside `Execute`/`TxExecute` the batch is scanned with `BatchTreeVisitor` (all leaves, including static `ForEach` bodies): admin leaves are denied; `Subscribe`/`Unsubscribe` leaves are denied on REST (no push channel; they would silently do nothing, review F17); `List::{Databases, Users}` and `AccessTree` are denied for tokens with a `databases` allowlist unless their `db` is inside it (review F4).
- Mode `ReadOnly` rejects any batch containing a write (`BatchOp::is_write()`, which classifies `Call` as a write, documented), rejects `TxExecute` write batches and `TxCommit` (review F18). The `databases` allowlist is checked against the `db` of every request and of every `TxBegin`.
- `ScopedHandler` first runs `validate_request_alias_caps` and decodes the request once, as `ShamirDbHandler::handle` does, and returns a typed outcome (`Allowed(response_bytes) | Denied(code) | NotSupported(code)`) so the REST layer maps `403`/`400` without decoding the response again (review F17, F21). Scope errors use the existing `DbResponse::Error` shape with codes `token_read_only`, `token_database_denied`, `token_request_denied`, `token_channel_denied`, `not_supported_over_rest`.
- The effective rights are the intersection of the owner's live rights (evaluated by the engine on every request) and the token scope.

## 7. Token administration

1. Offline CLI on the stopped data directory (the `shamir-server access-tree` pattern): `shamir-server token create --user U --label L --ttl 30d [--read-only] [--db NAME]...`, `token list [--user U]`, `token revoke <id>`, `token revoke-user U`, `token revoke-all`. `create` prints the secret once to a TTY, or to `--out-file` with mode 0600; never to a log.
2. Online wire operations over an authenticated SCRAM session (never over a token session: the token allowlist contains none of them, so a token cannot mint or manage tokens): `CreateApiToken`, `ListApiTokens`, `RevokeApiToken`, `RevokeUserTokens`, new `DbRequest` variants. **The online operations are required before the listeners are considered usable** (a leaked token must be revocable without stopping the server): T3/T6 may land earlier, but the end-to-end task depends on T5 and the feature is not documented as production-ready before T5.
3. **Who may issue tokens to whom (owner follow-up, revision 3).** Users get tokens for HTTP and WebSocket in two ways, both through the same operation:
   - **Self-service**: any authenticated user creates, lists and revokes their own tokens (`CreateApiToken { label, ttl, mode, channels, databases }` without `for_user`). No HMAC confirmation is needed (it only grants the user a subset of what they already have). Limits: `tokens.max_per_user` active tokens (default 20), `tokens.max_ttl_days`, label unique per owner; `mode`, `channels` and `databases` can only narrow, because the effective rights are always the intersection with the owner's live rights.
   - **Issued by an administrator**: a superuser creates a token for any user (`for_user: Some(name)`), lists any user's tokens, revokes any token, and runs `RevokeUserTokens`. These superuser operations carry the destructive-operation HMAC confirmation, like `create_scram_user`. This also covers service accounts: create a user, then issue them tokens, no human password needed beyond the account's own SCRAM record.
   - A user never sees another user's tokens; a listing never contains secrets, MACs or hashes (id, label, owner, mode, channels, databases, created, expires, revoked, last-presented time is not tracked in v1).
   - The secret is returned exactly once in the `CreateApiToken` response over the authenticated, TLS-protected (or loopback) SCRAM channel; it is never stored in clear and cannot be shown again.
   - Password change, role change, kick and restore bump `tokens_invalid_before_ns` for the user, so a password change revokes all of that user's tokens (they are re-issued afterwards); this is a security default (a changed password usually means a suspected compromise), see section 17.
4. The token store is node-local; online create/revoke run on a `ReadWrite` node, a read-only replica serves existing tokens from its own copy and manages them offline.

Every create, revoke and failed verification is an audit event on the HMAC chain with the token id (never the secret). Malformed-input and unknown-id failures are aggregated per subnet and sampled, not written one-for-one (review F20: pre-auth write amplification). Metrics carry only `result` and `channel` labels.

## 8. REST channel (channels 1 and 3)

| Method and path | Auth | Meaning |
|---|---|---|
| `POST /shamir/v1/request` | Bearer | body: one msgpack `DbRequest`; response: the msgpack `DbResponse` produced by the handler |
| `GET /shamir/v1/ping` | Bearer | liveness of the authenticated path (`DbRequest::Ping`) |

Everything else is `404`/`405`. No unauthenticated route exists on this listener. `Content-Type: application/msgpack` in and out.

**Authenticate before reading the body** (review F6): the token is verified from the request head; only then is the body read, under `max_request_body_bytes` (default: the wire data-frame cap) **and** a global in-flight request-body budget shared by all connections (so `max_active_connections x 16 MiB` of unauthenticated buffering is impossible). Head read, body read and total deadlines reuse the `auth_init_timeout` mechanism.

Request execution reproduces the invariants that live only in the SCRAM path (review F5): the request runs under `byte_budget::run_with_handoff` and the response-byte guard is held until the response body is written; the validity check and post-auth rate limit of `dispatch_request_view` are applied to the token session; the `application_error_boundary` (internal error redaction) wraps the handler. Each request gets a token session with a fresh random session id, cached per token id only for the rate-limit bucket.

Status mapping (HTTP status only for gateway-level outcomes; application errors keep the single `DbResponse::Error` taxonomy and travel with `200`):

| Situation | Status | Body |
|---|---|---|
| ok, including `DbResponse::Error` | 200 | msgpack `DbResponse` |
| missing/malformed/wrong/expired/revoked token | 401 | fixed msgpack `{"error":"unauthorized"}`, `WWW-Authenticate: Bearer` |
| scope violation (`token_read_only`, `token_database_denied`, `token_request_denied`, `token_channel_denied`) | 403 | `DbResponse::Error` |
| `not_supported_over_rest` | 400 | `DbResponse::Error` |
| unsupported method or path | 405 / 404 | fixed |
| body above the limit | 413 | fixed, connection closed |
| wrong `Content-Type` | 415 | fixed |
| rate limited | 429 | `Retry-After` |
| draining / overloaded | 503 | `Retry-After` |

HTTP framing rules (review F22): HTTP/1.1 only, no pipelining; reject a request that carries both `Content-Length` and `Transfer-Encoding`, duplicate or non-numeric `Content-Length`, obsolete header folding, a bare LF line ending, and `Transfer-Encoding` other than `chunked`; close the connection on any framing anomaly. Head at most 8 KiB and 64 headers. `Expect: 100-continue` is answered `417` (clients retry without it). Responses carry `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`, and on TLS `Strict-Transport-Security`. No CORS header.

## 9. WebSocket channel (channels 2 and 4)

### 9.1 Upgrade and session binding

`GET /shamir/v1/ws`, `Upgrade: websocket`, `Sec-WebSocket-Protocol: shamir-v1`, `Authorization: Bearer <token>`. The token is verified before `101`; failure answers `401` and no upgrade. `Origin` is optional; when present it must match the listener's `browser_origin_allowlist` (the existing field), and an empty allowlist rejects any request that carries `Origin`.

Immediately after the upgrade the server sends one binary frame `{ "ok": true, "sid": <32 random bytes>, "expires_at_ns": N }` (msgpack). All later frames are the usual `{sid, rid, req}` / `{rid, res}` envelopes; **the first frame's `sid` is the only `sid` the connection accepts** (finding F1). A token client that sends any other `sid` receives close `4010`.

### 9.2 Loop

The token request loop is `run_request_loop` with these differences: the session comes from the connection, not from a `SessionStore` lookup by frame `sid` (the frame `sid` is only compared); the dispatch task re-checks token state (`revoked`/`expired`/owner) with an Acquire load before calling the handler, so no request is authorized after the revoke call returned; a cancellation arm in the loop's `select!` tears the connection down when the token session's cancel handle fires; on cancel, subscription bridges owned by the session are unregistered (they capture an `Actor` and never re-check, review F2).

### 9.3 Close codes

New codes, registered in `TRANSPORT_WS.md` section 6 **before** T7 starts (review F2: `4003` is `heartbeat_timeout`): `4010 token_sid_mismatch`, `4011 token_revoked`, `4012 token_expired`.

### 9.4 Lifetime and accounting

A token session ends at `min(token.expires_at, token_session_max_age)` (the SCRAM `SESSION_MAX_AGE` of 24 h does not apply, but a separate configurable maximum does). Token sessions have their own caps (`tokens.max_sockets_per_token`, default 8, and a per-owner cap); they never consume the SCRAM per-user cap of 16, so a token holder cannot lock the owner's SCRAM logins out (review F9). Revocation, expiry, owner kick or role change fire the cancel handle; the socket closes within the same publish step.

## 10. Transport security

### 10.1 TLS (channels 3 and 4)

TLS 1.3 only, no 0-RTT, ALPN `http/1.1`, the same `TlsConfig` material. The existing justification for self-signed certificates is the application-level Ed25519 server pin, which a token client does not have, so for token listeners the certificate is the only server authentication (review F19): **a non-loopback TLS token listener refuses to start with an auto-generated certificate** unless `allow_self_signed: true` is set on that listener, in which case boot prints the certificate's SPKI SHA-256 pin for distribution. Operators who expose the port to third parties provide a CA-issued certificate.

### 10.2 Plaintext channels (channels 1 and 2)

- A listener with `tls: false` must bind a loopback address; `Config::validate` refuses anything else, as for `profile: plain` today.
- `allow_plaintext_remote: true` on that listener allows a non-loopback bind. Boot logs a WARN naming the address and the gauge `token_listener_plaintext_remote` is 1. Over such a listener **only `ReadOnly` tokens are accepted unless `allow_plaintext_write: true` is also set**; this is checked at token verification, from the listener's own configuration, not from the peer address (review F7: a peer-address rule is vacuous behind a proxy).
- Behind a reverse proxy every peer is the proxy, which would collapse per-IP counters and `max_active_connections_per_ip`. `trusted_proxies: [cidr...]` names the proxies whose `X-Forwarded-For` is honoured for rate limiting; the header is never trusted otherwise.
- This contradicts two statements in the current specs ("plain HTTP only loopback" in `ADMIN_UI_HOSTING.md` section 7.1 and "wss only" in `TRANSPORT_WS.md`); both are amended explicitly in the spec task T1, not left to the final documentation task.
- The token is a bearer secret; over plaintext it is visible to every hop. The operator guide and `SECURITY_MODEL.md` say so.

## 11. Logging and observability

- The `Authorization` header and any token substring are redacted at the earliest point; the HTTP layer never forwards the raw header into log fields, error strings or spans. A log-redaction test in the style of `shamir-connect/tests/audit/integration_log_redaction.rs` captures logs of failed and successful requests and asserts no token, secret or MAC appears.
- Token ids (never secrets) appear in audit events and debug logs.
- Metrics: `token_auth_total{channel,result}`, `token_active_ws`, request latency per endpoint; no per-token label.

## 12. Configuration

```
listeners: [
    {
        kind: http
        addr: 127.0.0.1:7340
        tls: false                       # channels 1 and 2 (loopback only)
        endpoints: [rest, ws]            # default both
    }
    {
        kind: http
        addr: 0.0.0.0:7341
        tls: true                        # channels 3 and 4; top-level tls: { cert_path, key_path }
        endpoints: [rest, ws]
        max_request_body_bytes: 16777216
        browser_origin_allowlist: []     # existing field, WebSocket Origin check
        allow_self_signed: false
        allow_plaintext_remote: false    # tls: false only
        allow_plaintext_write: false     # tls: false only
        trusted_proxies: []
    }
]
tokens: {
    max_ttl_days: 365
    default_ttl_days: 30
    max_per_user: 20
    allow_self_service: true         # false = only superusers issue tokens
    max_sockets_per_token: 8
    session_max_age_hours: 24
}
```

`tls` is required for `kind: http` and rejected for the other kinds; `ListenerConfig.profile` becomes `Option` (required for tcp/ws/unix, absent for http) — the validator enforces it (review F16). Exact key names are fixed in task T6 and shown in `deploy/server.example.ktav`.

## 13. Interaction with existing features

- Replication, bootstrap, user admin and `access-tree` are untouched; `access-tree` gains a token section in T4.
- `auth_message` encoding is unchanged because tokens never take part in a SCRAM transcript and the wire enums are not extended (rule 3).
- Resumption tickets: a token session neither receives nor presents tickets (the ticket code requires `origin == Scram`).
- Backup/restore: restore bumps `tokens_invalid_before_ns` for all users (section 6.3) and the restore runbook lists `token revoke-all` for a restore from an untrusted backup.

## 14. Relation to `ADMIN_UI_HOSTING.md`

The REST sketch there (session id as Bearer, admin commands) is superseded for REST access by this design: REST clients use API tokens and a token can never be an admin. The admin-command part of that sketch (`/shamir/v1/admin/<command>`) is explicitly left unimplemented and is marked so in the spec; the static admin SPA hosting is unchanged and out of scope. A browser SPA calling the token REST API is not supported in v1 (no CORS, section 2).

## 15. Test plan

Unit: token parser (all malformed shapes, huge input, non-UTF8, duplicate headers, separator confusion), MAC/verify (constant-time helper, dummy-MAC path, `key_id`/`format_version` in the MAC input), expiry/revocation/owner-state transitions (re-created account does not inherit tokens, restore bump), scope classifier over every `DbRequest` variant and every `BatchOp` variant (exhaustive, deny-by-default for new variants), config validation (plaintext + non-loopback refused, opt-ins, `tls` mandatory, self-signed refusal), HTTP framing rules (CL+TE, duplicate CL, obs-fold, bare LF, oversized head).

Integration (server crate, real sockets, loopback, OS ephemeral ports):
- the matrix {channel 1, 2, 3, 4} x {valid read, valid write, read-only token writing, database outside the allowlist, wrong secret, unknown id, expired, revoked after connect (WS closes 4011), owner kicked, oversized body, header flood, slow head, missing header, token in query string ignored, two Authorization headers};
- **cross-listener replay**: a token `sid` sent over a SCRAM connection is rejected, a SCRAM `sid` sent over a token connection is rejected (4010); token sessions never appear in the SCRAM `SessionStore`;
- **privilege**: superuser-owned and replicator-owned tokens against every admin `DbRequest`, every admin `BatchOp`, `Repl`, `ChangePassword*`, `List::{Databases,Users}`, `AccessTree`: all denied;
- **subscription**: a token WebSocket with a live subscription stops receiving after revoke, without the client sending anything;
- **race**: revoke while requests are in flight; no request authorized after the revoke call returned (T5 provides the online revoke);
- **accounting**: 16 active token sockets of one owner do not block that owner's SCRAM login;
- **budget**: pre-auth body flood does not exceed the global body budget; response-byte guard held until the body is written;
- **issuance**: a user creates/lists/revokes only their own tokens; `for_user` by a non-superuser is rejected; a superuser issues for another user with the HMAC confirmation and not without it; `max_per_user` and TTL caps; `allow_self_service: false` rejects non-superuser creation; a token session cannot call any token operation; a password change revokes the user's tokens; the secret appears exactly once and never in a listing;
- **channels**: a `Rest`-only token is refused at the WebSocket upgrade (403, no `101`), a `Ws`-only token is refused on REST (`token_channel_denied`), `Both` works on both;
- log-redaction test; restore bumps tokens; WebSocket subscription push and cursors through a token session; REST `not_supported_over_rest`.
Windows and Linux in CI; no sleeps (lifecycle seams).

Property/fuzz-style (cheap, in unit tests): random byte strings through the header parser never panic and never authenticate.

## 16. Work breakdown (tasks)

Each task becomes a committed brief under `docs/dev-artifacts/prompts/remediation/` (prompt-first) when it starts; CI runs in the fork; landing in upstream follows the standing procedure. TaskList ids in brackets.

| Id | Task | Crates | Depends on |
|---|---|---|---|
| T0 | Independent review of this document, folded into revision 2 (done) [#1734] | docs | - |
| T0b | Make room: split the folders and files named in section 5 so the later tasks do not violate the layout limits | shamir-server, shamir-connect | - |
| T1 | Normative spec first: `TOKEN_API.md` (statuses, close codes, error codes, envelopes, token format), explicit amendments to `TRANSPORT_WS.md`, `ADMIN_UI_HOSTING.md`, `SECURITY_MODEL.md`, `IMPLEMENTATION_GUIDE.md` | docs | T0 |
| T2 | Token model (including `channels`), `ApiTokenStore`, HMAC verification, in-memory mirror and owner state, `TokenSessionStore`, `SessionOrigin`, fjall store, `token_key` [#1735] | shamir-connect, shamir-server | T0b, T1 |
| T3 | `shamir-transport-http` crate and REST channel (HTTP and HTTPS): listener kind, plaintext rails, framing rules, auth-before-body, `ScopedHandler`, status mapping, budgets, redaction [#1737] | shamir-transport-http, shamir-server, shamir-connect | T2 |
| T4 | Offline CLI `shamir-server token ...`, audit events, `access-tree` section [#1736] | shamir-server | T2 |
| T5 | Online administration and issuance: `CreateApiToken` (self-service and `for_user` by a superuser), list, revoke, revoke-user as `DbRequest` variants, handler with the HMAC gate for superuser operations, per-user limits, password-change bump, builders (Rust, TS), client methods, spec tables [#1739] | shamir-query-types, shamir-query-builder, shamir-server, shamir-client, shamir-client-ts | T2, T4 |
| T6 | WebSocket token channel (WS and WSS): upgrade via `hyper::upgrade`, session binding frame, token request loop with cancel arm, close codes, subscription teardown [#1738] | shamir-transport-http, shamir-server | T2, T3 |
| T7 | End-to-end matrix, redaction test, revoke race, privilege/replay/accounting tests, Windows+Linux in the fork CI [#1740] | shamir-server | T3, T5, T6 |
| T8 | Final documentation: operator guide, `deploy/*.ktav` examples, `KNOWN_LIMITATIONS.md`, `CLAUDE.md` crate list (the only place it changes) [#1741] | docs | T3, T5, T6, T7 |
| T9 | (deferred) token connect option in `shamir-client` and `shamir-client-ts`, JSON bodies decision, browser WebSocket token carrier, mTLS [#1742] | - | T7 |

T3 and T6 are the large stages; each starts with its own design note (module split, dependency choice, memory-ordering argument for the revocation publish) before code, in the style of the 07b stage.

## 17. Open decisions (defaults chosen; change them before T3 starts)

1. **Plaintext policy.** Default here: loopback-only plaintext; a non-loopback bind needs `allow_plaintext_remote`, read-only tokens only unless `allow_plaintext_write` is also set. Alternative: forbid remote plaintext completely.
2. **Meaning of "certificate".** Default here: TLS server certificate (https/wss). Alternative or addition: client certificates (mTLS).
3. **Body encoding.** Default here: msgpack only, as in the whole wire protocol (JSON does not round-trip the engine's numeric and binary types, see `NUMERIC_WIRE_SEMANTICS.md`). Alternative: a JSON mode with documented lossy types, for curl-friendliness.
4. **Token and superuser.** Default here: a token never carries admin rights. Alternative: an explicit `admin` scope for superuser-owned tokens.
5. **Expiry.** Default here: mandatory (max `tokens.max_ttl_days`). Alternative: non-expiring tokens.
6. **`token_key` rotation.** Default here: none in v1 (re-issue after a leak). Alternative: multi-key scheme with `key_id` lookup.
7. **Who issues.** Default here: self-service for every user plus issuance by superusers (`tokens.allow_self_service: true`). Alternative: only superusers issue tokens.
8. **Channel scope.** Default here: every token has a `channels` field (`rest`, `ws`, `both`; default `both`) and is refused on the other endpoint. Alternative: no per-channel scope.
9. **Password change.** Default here: a password change revokes all of the user's tokens. Alternative: tokens survive a password change.

## 18. Risks

- A bearer token over plaintext is stealable by any on-path observer; the rails reduce accidental exposure but cannot remove it.
- A new network-facing HTTP driver is new attack surface; mitigations are hyper (no hand-written parser), hard limits, auth-before-body, the framing rules and the fuzz-style tests.
- Revocation consistency between the persisted store, the in-memory mirror, live sockets and in-flight requests is a lifecycle race class already seen in the client (`ClosedState`); T2/T6 designs must argue the ordering explicitly (publish, then cancel; per-request Acquire re-check).
- The token request loop is a second variant of the post-auth loop; drift between the two copies is a maintenance risk. T6 factors the shared parts instead of copying them.
- Adding a workspace crate touches CI scripts and `CLAUDE.md`; the cooldown job gates any new dependency.

## 19. Review traceability

Review F1 (sid binding), F2 (revoke closes sockets, close codes), F3 (permissions constructed, deny-by-default) are HIGH and are the rules in section 4 and sections 6.4 and 9. F4 (metadata leak) in 6.4. F5 (REST invariants) and F6 (pre-auth body) in section 8. F7 (plaintext rails) in 10.2. F8 (wire enums) in rule 3. F9 (accounting) in 9.4. F10 (key rotation) and F15 (separator) in 6.1 and 6.2. F11 (restore) in 6.3 and 13. F12 (owner validity, lockout) in 6.3. F13 (hyper and upgrade) in 5 and 9. F14 (task order, layout) in 5 and 16. F16 (config), F17 (subscribe, typed outcome), F18 (read-only gaps), F19 (self-signed), F20 (audit amplification), F21 (alias caps), F22 (framing), F23 (spec contradictions), F24 (HMAC unreachable) in the sections named above. Unverified by the reviewer and carried into the task designs: whether the engine re-checks an `Actor` per subscription event, whether `auth_init_timeout` is reusable as a head timeout, replica behaviour of the store, and the exact CI-script edits for a new crate.
