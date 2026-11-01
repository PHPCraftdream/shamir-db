# Token API: REST and WebSocket access by bearer token

Status: DESIGN (2026-11-01). Requested by the owner on 2026-11-01. Nothing in this document is implemented. The work is split into tasks listed in section 16; the first task is an independent security review of this document.

## 0. Request and interpretation

Owner request: expose a REST API to the outside, authenticated by a token in the request headers, over four channels:

| # | Channel | Meaning |
|---|---|---|
| 1 | HTTP + Bearer | plain HTTP, `Authorization: Bearer <token>` |
| 2 | WebSocket | plain WebSocket, the token in the upgrade request headers |
| 3 | channel 1 + certificate | HTTPS (TLS server certificate) |
| 4 | channel 2 + certificate | WSS (TLS server certificate) |

Interpretation used here (to be confirmed, see section 17): "certificate" means the TLS server certificate that `https` and `wss` need. Client certificates (mutual TLS) are not part of this design.

## 1. Goals and acceptance criteria

1. A third-party program that has only a token string can call the database over HTTP/1.1 (`POST`, msgpack body) and over a WebSocket, on all four channels, with no SCRAM handshake and no database client library.
2. Every request runs as a real database principal: the existing authorization (owner/group/mode access, `Actor`, read-only node gate, query limits, byte budget, rate limits) applies unchanged. A token can never do more than its owner could do over a SCRAM session, and by default does less (section 6.4).
3. Tokens are issued, listed and revoked by an administrator; revocation is effective for the next request and closes live token WebSockets.
4. A plaintext channel cannot be opened by accident: it is loopback-only unless the operator sets an explicit opt-in (section 10.2).
5. A wrong, expired, revoked or malformed token yields one uniform `401` without timing or body differences, and never appears in logs, metrics labels, audit records or error text.
6. Observable proof: an end-to-end test matrix of 4 channels x {valid, wrong, expired, revoked, read-only violation, oversized body, missing header} (section 15) is green in CI on Linux and Windows.

## 2. Non-goals (this design)

- Mutual TLS, OAuth/OIDC, JWT, cookies, sessions by cookie, CSRF protection (Bearer in a header has no ambient authority).
- JSON request bodies (deferred, section 17.3), a resource-style REST surface (`/tables/...`), OpenAPI generation.
- Browser WebSocket clients (browsers cannot set the `Authorization` header on a WebSocket; a subprotocol-carried token is a separate decision).
- Admin operations over a token (user admin, superuser/replicator flags, password change, replication); these stay on SCRAM sessions.
- Interactive transactions, cursors and subscriptions over REST (they need a server-side session); they are available on the WebSocket channels.
- Per-token quotas beyond the rate limit, token usage statistics persisted per request, token scopes beyond section 6.4.
- Static hosting of the admin UI (it stays specified in `ADMIN_UI_HOSTING.md`; this design reuses its REST sketch and replaces its "session id as Bearer" with API tokens, section 14).

## 3. What exists today (verified in the tree at `cba8515e`)

- Transports: TCP+TLS (`shamir-transport-tcp`), WebSocket over TLS (`shamir-transport-ws`, `/shamir/v1` native and `/shamir/v1/browser`), local IPC. All authenticate with SCRAM-Argon2id plus channel binding, then run the same post-handshake loop (`shamir-server/src/sync/connection/request_loop.rs`, the `Framer` abstraction in `sync/framer.rs`, `RequestHandler::handle(&Session, req_bytes, &ConnectionServices)` implemented by `ShamirDbHandler`).
- Plain WebSocket does not exist (`TRANSPORT_WS.md`: "plain WS removed in v1"). Plain TCP exists as `ProfileKind::Plain`, refused on a non-loopback address by `Config::validate`.
- Listener configuration: `ListenerConfig { kind: tcp|ws|unix, addr, profile: tls_exporter|tls_no_export|plain, path, ... }` in `shamir-server/src/boot/config.rs`; TLS material `TlsConfig { cert_path, key_path }`, generated self-signed on first start if both files are absent (`security/tls.rs`).
- An HTTP/1.1 server already exists for observability (`axum` 0.7, features `http1`+`tokio`): `/healthz /readyz /metrics /info`, default `127.0.0.1:9090`, non-loopback needs `allow_public_metrics`.
- `ADMIN_UI_HOSTING.md` sketches `POST /shamir/v1/query` with `Authorization: Bearer <session_id>` and msgpack bodies, plain HTTP "loopback only", HTTPS otherwise. It is a specification only; no code serves it.
- Authorization context: `session_actor(session)` yields `Actor::Admin(principal64)` for superusers and `Actor::User(principal64)` otherwise (`engine/db_handler/handler.rs`). The read-only gate for replicas classifies batches with `BatchTreeVisitor` and `BatchOp::is_write()`.
- WebSocket upgrade header inspection already exists (`tokio_tungstenite::accept_hdr_async_with_config` with an `Origin` callback in `shamir-transport-ws/src/server.rs`).

## 4. Architecture

```
            channel 1/3                       channel 2/4
   curl/any HTTP client --POST--+   +--GET Upgrade-- any WS client
                                |   |
                       +--------v---v---------+   plaintext (1,2) or TLS 1.3 (3,4)
                       |  token listener      |   one port serves both endpoints
                       |  (kind: http)        |
                       +----------+-----------+
                                  | Authorization: Bearer <token>
                          +-------v--------+
                          | TokenVerifier  |  lookup by id -> constant-time hash compare
                          | (ApiTokenStore)|  expiry, revoked, owner still valid, scope
                          +-------+--------+
              REST: ephemeral Session | WS: Session for the socket lifetime
                                  |
                  +---------------v----------------+
                  | ScopedHandler (token scope)    |  read-only / database allowlist / REST allowlist
                  +---------------+----------------+
                                  |
                       RequestHandler::handle(...)   (existing ShamirDbHandler, unchanged)
```

Rules of the architecture:

1. The token path never reaches SCRAM, Argon2 or the resumption-ticket machinery. A token session must not be able to mint or resume a ticket, change a password or hold superuser rights (section 6.4).
2. `ShamirDbHandler` is not modified for authentication. Scope enforcement is a wrapper (`ScopedHandler`) around the handler, so the handler keeps one meaning.
3. The WebSocket channel reuses `request_loop` through the `Framer` abstraction; only session creation differs from `handle_connection`.
4. One listener kind serves both endpoints (`rest`, `ws`) so the four channels are the product of two switches: `tls` and `endpoints`.

## 5. Crate and module layout

- New crate `shamir-transport-http` (family of `shamir-transport-tcp/ws/ipc`): HTTP/1.1 request-head parsing with hard limits, TLS accept (reusing `shamir-transport-tcp::tls`), `Authorization` header extraction and validation, WebSocket upgrade with a header hook (reusing `tokio-tungstenite` 0.30 already in the tree). No dependency on `shamir-db`. Adding a workspace crate requires updating the crate list in `CLAUDE.md`, `scripts/ci` crate maps and the affected-crates script; the task covers it.
- `shamir-connect` (server side): `ApiTokenStore` trait, token model and verification (pure, no I/O), next to `UserDirectory`.
- `shamir-server`: the fjall-backed store (`security/user_directory/` neighbour, new keyspace `api_tokens_v1`), `ListenerKind::Http` wiring in `boot/config.rs` and `server_launcher/`, `ScopedHandler`, the token session constructor, CLI subcommands, metrics and audit hooks.
- `shamir-query-types`, `shamir-query-builder`, `shamir-client`, `shamir-client-ts`: the wire operations for token administration (task T5, after the server side is accepted).
- A new dependency is allowed only if it passes `cargo deny` and the 30-day cooldown job; prefer what is already in the tree (`hyper`/`http` via `axum`, `tokio-tungstenite`, `rustls`, `blake3` or `sha2` if already used). Decide in T3 and state it in its design note.

## 6. Tokens

### 6.1 Format

`shk1_<id>_<secret>` where `id` is 16 random bytes and `secret` is 32 random bytes from the OS CSPRNG, both base64url without padding. The prefix `shk1_` makes leaked tokens greppable by secret scanners and versions the format. The secret is shown exactly once, at creation.

### 6.2 Storage

Persisted record (msgpack, keyed by `id`) in a new fjall keyspace `api_tokens_v1`, with a secondary index `user_id -> [id]`:

```
id: [u8;16]
owner_user_id: [u8;16]          // the principal the token acts as
label: String                   // operator-chosen, unique per owner, not secret
secret_hash: [u8;32]            // keyed BLAKE3(server token key, secret); never the secret
created_at_ns, expires_at_ns    // expiry is mandatory; the server enforces a configured maximum lifetime
mode: ReadOnly | ReadWrite
databases: Option<Vec<String>>  // allowlist; None = every database the owner may access
revoked_at_ns: Option<u64>
format_version: u8
```

The secret is high-entropy, so a fast keyed hash is correct; Argon2 here would be a CPU-exhaustion lever against the server. The key is a new `token_key` in `server_meta` (same lifecycle as `ticket_key`: created at bootstrap, rotation with overlap, included in the compromise runbooks). A snapshot of the user store alone therefore does not allow offline verification of guesses.

### 6.3 Verification (hot path, lock-free)

An in-memory `scc::HashMap<[u8;16], Arc<TokenState>, THasher>` mirrors the persisted records (loaded at boot, updated on create/revoke/owner change). Per request:

1. Parse exactly one `Authorization` header, scheme `Bearer` (case-insensitive), no other source of tokens: never the query string, a cookie or a body field.
2. Split the token; reject malformed input with the same `401` as a wrong token.
3. Look up `id`. When the id is unknown, still compute a keyed hash of the presented secret against a dummy so the unknown-id and wrong-secret paths take comparable time.
4. Constant-time compare the computed hash with `secret_hash`.
5. Check `revoked_at`, `expires_at`, and that the owner still exists, is not locked out and has not been kicked (`tickets_invalid_before_ns` semantics reused as `tokens_invalid_before_ns`: kicking a user or changing their roles invalidates their tokens' live sockets and forces re-evaluation of REST requests).
6. Build the effective principal (section 6.4) and the scope.

Memory ordering and cache invalidation of the mirror follow the lifecycle pattern already used in `shamir-client` (`ClosedState`): publish the revocation first, then close live sockets.

### 6.4 Principal and scope

- A token acts as its owner: `Actor::User(principal64(owner))`. A token never acts as `Actor::Admin`, even when the owner is a superuser. Everything that needs the Shomer-gate bypass or the admin glue (create user, set superuser, set replicator, change password, replication requests) is therefore unreachable by token. This is the first design decision to confirm (section 17.4).
- Token scope: `mode = ReadOnly` rejects any batch containing a write, classified by the existing `BatchTreeVisitor` + `BatchOp::is_write()` visitor (the code that implements the read-only replica gate), and rejects `TxCommit`; `databases` restricts the `db` field of every request. Scope is checked by `ScopedHandler` before the handler runs; a rejection returns the existing `DbResponse::Error` shape with new codes `token_read_only` and `token_database_denied`.
- The effective rights are the intersection of the owner's rights (evaluated live by the engine on every request) and the token scope. If the owner loses a right, the token loses it at once.
- Token sessions are marked with a new `TransportKind::HttpToken` and `BindingMode::None`; the resumption code refuses to issue or accept tickets for them.

### 6.5 Rate limiting and abuse

- Per source IP: a failure counter feeding the existing `RateLimiter`/`LockoutStore` (subnet granularity as for SCRAM, constant-time responses); repeated failures from one subnet back off with `429` and `Retry-After`. Success never resets another subnet's counters.
- Per token: a post-auth token bucket reusing `Session::post_auth_bucket` semantics (REST: the ephemeral session is cached per token id for the bucket's lifetime so the bucket survives across requests).
- Global: `max_active_connections` and `max_active_connections_per_ip` apply to the token listener; pre-auth deadlines (head read timeout, upgrade timeout) reuse the `auth_init_timeout` mechanism.

## 7. Token administration

Two surfaces, both required before the listener is considered usable:

1. Offline CLI on the stopped data directory (same pattern as `shamir-server access-tree`): `shamir-server token create --user U --label L --ttl 30d [--read-only] [--db NAME]...`, `token list [--user U]`, `token revoke <id>`, `token revoke-user U`. `create` prints the secret once to the terminal, never to a log, and refuses to write it to a non-TTY unless `--out-file` with mode 0600 (the bootstrap-token precedent).
2. Online wire operations over an authenticated SCRAM session: `CreateApiToken`, `ListApiTokens`, `RevokeApiToken`, `RevokeUserTokens` as new `DbRequest` variants, gated like `create_scram_user` (superuser plus the destructive-operation HMAC confirmation; a user may list and revoke only their own tokens). Builders in `shamir-query-builder` and `shamir-client-ts` follow in the client task.

Every create, revoke and failed verification is an audit event on the existing HMAC chain with the token id (never the secret); the metrics counters carry only the result label (`ok`, `unauthorized`, `forbidden_scope`, `rate_limited`).

## 8. REST channel (channels 1 and 3)

Routes, one listener, `Content-Type: application/msgpack` in and out, the same encoding as the WebSocket binary frames:

| Method and path | Auth | Meaning |
|---|---|---|
| `POST /shamir/v1/request` | Bearer | body: one msgpack `DbRequest`; response: the msgpack `DbResponse` produced by `ShamirDbHandler::handle` |
| `GET /shamir/v1/ping` | Bearer | liveness of the authenticated path (maps to `DbRequest::Ping`) |

Everything else is `404`. No unauthenticated route exists on this listener (health stays on the observability server).

`POST /shamir/v1/request` accepts only an allowlist of request variants: `Ping`, `Execute`, `WaitForVisibility`, `GetDdlOpStatus`. Interactive transaction, cursor, subscription, admin and replication variants return `400` with `DbResponse::Error { code: "not_supported_over_rest" }`; they are available on the WebSocket channels (cursors and transactions need a server session).

Status mapping (HTTP status only for gateway-level outcomes; application errors keep the single `DbResponse::Error` taxonomy and travel with `200`):

| Situation | Status | Body |
|---|---|---|
| ok, including `DbResponse::Error` | 200 | msgpack `DbResponse` |
| missing/malformed/wrong/expired/revoked token | 401 | fixed msgpack `{"error":"unauthorized"}`, header `WWW-Authenticate: Bearer` |
| token scope violation | 403 | `DbResponse::Error` with `token_read_only` / `token_database_denied` |
| unsupported method or path | 405 / 404 | fixed |
| body above the limit | 413 | fixed, connection closed |
| wrong `Content-Type` | 415 | fixed |
| rate limited | 429 | `Retry-After` |
| server shutting down / overloaded | 503 | `Retry-After` |

Limits: request head at most 8 KiB, at most 64 headers, body at most `max_request_body_bytes` (default equal to the wire data-frame cap), idle and total deadlines per request, HTTP/1.1 only (no HTTP/2, no pipelining, `Connection: close` after any error), no `Expect: 100-continue` handling beyond refusal, `Transfer-Encoding: chunked` request bodies are read under the same byte cap. Responses carry `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, and on TLS `Strict-Transport-Security`. CORS is off by default (no `Access-Control-*` headers); an `allowed_origins` list may enable preflight for named origins without credentials.

A REST request creates an ephemeral `Session` (cached per token id for the rate-limit bucket), calls the handler once, and drops it. No server state survives the request.

## 9. WebSocket channel (channels 2 and 4)

- Endpoint `GET /shamir/v1/ws`, `Upgrade: websocket`, `Sec-WebSocket-Protocol: shamir-v1`, `Authorization: Bearer <token>` on the upgrade request. The token is verified before `101 Switching Protocols`; failure answers `401` and no upgrade.
- `Origin`: optional (native clients), and when present it must match the listener's `allowed_origins`; an absent allowlist rejects any request that carries `Origin` (browsers are out of scope).
- After the upgrade: binary msgpack frames only, the same `{rid, req}` / `{rid, res}` envelopes, caps, ping/pong and close codes as `TRANSPORT_WS.md` (plus close code `4003 token_revoked` and `4004 token_expired`), driven by the existing `request_loop` through a `Framer` over the (plain or TLS) stream.
- The session lives for the socket. Revocation, expiry, owner kick or a role change closes the socket from the server side within one revocation-publish step. The token's `expires_at` bounds the socket lifetime.
- All request variants the owner may use are available except the admin set removed in section 6.4. Subscriptions, cursors and interactive transactions work because a real session exists.

## 10. Transport security

### 10.1 TLS (channels 3 and 4)

TLS 1.3 only, no 0-RTT, ALPN `http/1.1`, the same `TlsConfig` material and the same self-signed-on-first-start behaviour as the other listeners. Unlike the SCRAM channels, no application-level server identity pin protects this path, so the certificate is the only server authentication: operators who expose the port to third parties must provide a CA-issued certificate or distribute their CA to clients. The document that ships with the feature says so, and `SECURITY_MODEL.md` gets the corresponding rows (a token stolen from the wire or a logged header is a bearer credential: it works from anywhere until it expires or is revoked).

### 10.2 Plaintext channels (channels 1 and 2)

- A listener with `tls: false` must bind a loopback address; `Config::validate` refuses anything else, as it does for `profile: plain` today.
- The only way to bind a plaintext token listener to a non-loopback address is `allow_plaintext_remote: true` on that listener (the same shape as `allow_public_metrics`). Boot then logs a WARN naming the address, and the metrics gauge `token_listener_plaintext_remote` is 1. The intended use is a trusted segment or a TLS-terminating reverse proxy in front.
- A plaintext listener never serves a token whose scope is `ReadWrite` unless `allow_plaintext_remote` is set or the peer address is loopback. (Defence in depth: remote plaintext can only carry read-only tokens by default.)
- The token is a bearer secret; over plaintext it is visible to every hop. This risk is stated in the operator guide and in `SECURITY_MODEL.md`.

## 11. Logging, audit, observability

- The `Authorization` header and any token substring are redacted at the earliest point: the HTTP layer never forwards the raw header into log fields, error strings or spans. A regression test in the style of `shamir-connect/tests/audit/integration_log_redaction.rs` captures logs of failed and successful requests and asserts that no token, secret or hash appears.
- Token ids (not secrets) appear in audit events and in debug logs.
- Metrics: `token_auth_total{channel,result}`, `token_active_ws`, request latency histogram per endpoint; no per-token label (cardinality and enumeration).

## 12. Configuration

```
listeners: [
    {
        kind: http
        addr: 127.0.0.1:7340
        tls: false                 # channels 1 and 2 (loopback only)
        endpoints: [rest, ws]      # default both
    }
    {
        kind: http
        addr: 0.0.0.0:7341
        tls: true                  # channels 3 and 4; uses the top-level tls: { cert_path, key_path }
        endpoints: [rest, ws]
        max_request_body_bytes: 16777216
        allowed_origins: []
    }
]
tokens: {
    max_ttl_days: 365
    default_ttl_days: 30
}
```

`tls` is required for `kind: http` and rejected for the other kinds. `endpoints` defaults to both. The existing `profile` field is not used by this kind. Exact key names are fixed in task T3 and shown in `deploy/server.example.ktav`.

## 13. Interaction with existing features

- Replication, bootstrap, user admin and `access-tree` are untouched. `access-tree` gains a token listing section in T2.
- `Session::transport_kind` gets one new value; every `match` over `TransportKind` is updated (compile-checked). `auth_message` encoding is unchanged because tokens never take part in a SCRAM transcript.
- Resumption tickets: a ticket issued to a SCRAM session is unaffected; token sessions neither receive nor present tickets.

## 14. Relation to `ADMIN_UI_HOSTING.md`

That document's "session id as Bearer" REST sketch is replaced by this design for REST access; an admin SPA that wants REST calls uses an API token like any other client. The static-UI part of that document is out of scope here and unchanged. T7 reconciles the text.

## 15. Test plan

Unit: token parser (all malformed shapes, huge input, non-UTF8, duplicate headers), hash/verify (constant-time compare helper, dummy-hash path), expiry and revocation transitions, scope classifier (read-only vs every `BatchOp` variant through the exhaustive `is_write`), config validation (plaintext + non-loopback refused, opt-in accepted, `tls` mandatory).

Integration (server crate, real sockets, `127.0.0.1:0`): the matrix {channel 1, 2, 3, 4} x {valid read, valid write, read-only token writing, database outside the allowlist, wrong secret, unknown id, expired, revoked after connect (WS closes with 4003), owner kicked, oversized body, header flood, slow head (slowloris), missing header, token in query string ignored, two Authorization headers}; log-redaction test; a concurrency test that revokes while requests are in flight (no request authorized after the revoke call returned); WebSocket subscription push and cursor through a token session; REST `not_supported_over_rest` for the excluded variants. Windows and Linux in CI; loopback and OS ephemeral ports only; no sleeps (use the lifecycle seams).

Property/fuzz-style (cheap, in unit tests): random byte strings through the header parser never panic and never authenticate.

## 16. Work breakdown (tasks)

Each task becomes a committed brief under `docs/dev-artifacts/prompts/remediation/` (prompt-first) when it is started; CI runs only in the fork; landing in upstream follows the standing procedure.

| Id | Task | Crates | Depends on |
|---|---|---|---|
| T0 | Independent security and architecture review of this document (read-only), fixes folded into this file | docs | - |
| T1 | Token model, `ApiTokenStore` trait, verification, in-memory mirror, fjall store and `token_key` in `server_meta`, unit tests | shamir-connect, shamir-server | T0 |
| T2 | Offline CLI `shamir-server token ...`, audit events, `access-tree` section | shamir-server | T1 |
| T3 | `shamir-transport-http` crate and REST channel on plain + TLS: listener kind, config validation with plaintext rails, head parser limits, auth, `ScopedHandler`, ephemeral sessions, status mapping, security headers, rate limits, redaction | shamir-transport-http, shamir-server, shamir-connect | T1 |
| T4 | WebSocket token channel on plain + TLS: upgrade hook, `Framer` over the stream, token session, close on revoke/expiry, subscriptions/cursors/tx through `request_loop` | shamir-transport-http, shamir-server | T1, T3 (shared listener) |
| T5 | Online token administration: `DbRequest` variants, handler, HMAC gate, Rust builder, TS builder, client methods, wire-spec tables | shamir-query-types, shamir-query-builder, shamir-server, shamir-client, shamir-client-ts | T1, T2 |
| T6 | End-to-end matrix (channels 1-4), log-redaction test, revocation race test, Windows/Linux in the fork CI | shamir-server | T3, T4 |
| T7 | Documentation: `TOKEN_API.md` normative spec in the protocol-spec folder, reconcile `ADMIN_UI_HOSTING.md`, `SECURITY_MODEL.md` rows, `TRANSPORT_WS.md` plaintext note, operator guide and `deploy/*.ktav` examples, `CLAUDE.md` crate list, `KNOWN_LIMITATIONS.md` | docs | T3, T4, T5 |
| T8 | (follow-up, not scheduled) token connect option in `shamir-client` and `shamir-client-ts`, JSON bodies decision, browser WebSocket token carrier, mTLS | - | T6 |

T3 and T4 are the large stages; each starts with its own design note (module split, dependency choice, memory-ordering argument for the revocation publish) before code, in the style of the 07b stage.

## 17. Open decisions (defaults chosen; change them before T3 starts)

1. **Plaintext policy.** Default here: loopback-only plaintext, non-loopback only with `allow_plaintext_remote: true` and read-only tokens by default. Alternative: forbid remote plaintext completely.
2. **Meaning of "certificate".** Default here: TLS server certificate (https/wss). Alternative or addition: client certificates (mTLS) as a second factor or as the credential.
3. **Body encoding.** Default here: msgpack only, as in the whole wire protocol (JSON does not round-trip the engine's numeric and binary types, see `NUMERIC_WIRE_SEMANTICS.md`). Alternative: a JSON mode with documented lossy types, for curl-friendliness.
4. **Token and superuser.** Default here: a token never carries admin rights. Alternative: allow an explicit `admin` scope for superuser-owned tokens.
5. **Expiry.** Default here: expiry is mandatory (max `tokens.max_ttl_days`). Alternative: allow non-expiring tokens.

## 18. Risks

- A bearer token over plaintext is stealable by any on-path observer; the rails in section 10.2 reduce accidental exposure but cannot remove it.
- A new network-facing parser (HTTP head) is new attack surface; mitigations are the hard limits, the reuse of `hyper`/`http` instead of a hand-written parser (T3 decides, preferring the library), and the fuzz-style tests.
- Token revocation consistency between the persisted store, the in-memory mirror and live sockets is a lifecycle race class already seen in the client (`ClosedState`); T1/T4 designs must argue the ordering explicitly.
- Adding a workspace crate touches CI scripts and `CLAUDE.md`; the cooldown job gates any new dependency.
