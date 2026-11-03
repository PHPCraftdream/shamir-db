# API tokens and token-based connections

This guide covers API-token issuance, Node/browser token connections, and password login over HTTP for `@shamir/client`. A token is a bearer credential: anyone possessing it can use its scope until expiry or revocation. Node APIs are exported from `@shamir/client`; browser APIs from `@shamir/client/browser`. `connectLocal` is Node-only. Older protocol-spec status banners saying these features are “not implemented” are stale; this guide follows current source.

## Issue tokens

### Offline CLI

The `shamir-server token` command family operates on a stopped server data directory. Put `--config` before `token`. The maintenance lock is required even for `list`; offline operations can be used on a replica. Tokens are node-local and are not replicated.

```sh
shamir-server --config server.ktav token create --user report-reader --label reporting-service --ttl 1d --channels http --db reports --out-file reporting.token
shamir-server --config server.ktav token list --user report-reader
shamir-server --config server.ktav token revoke '<TOKEN_ID>'
shamir-server --config server.ktav token revoke-user report-reader
```

`--channels` accepts `http`, `ws`, or `both` (default `both`); SDK issuance uses `rest` instead of CLI `http`. Creation is read-only unless `--read-write` is supplied. Repeat `--db` for a database allowlist; omission means all owner-accessible databases. `--ttl` takes a positive duration with `s`, `m`, `h`, or `d`. Server-configured ordinary defaults are 30 days, a 365-day maximum, and 20 active tokens per owner shared across issuance paths.

The CLI `create` command requires a terminal unless `--out-file` is provided; the output file must be new. This requirement does not apply to `ApiToken` or online `client.apiTokens.create`. Unix mode is 0600. Windows ACLs are not tightened; CLI warns, so secure file/directory yourself. Revoke commands do not rotate the server key.

### Online SCRAM administration

`ShamirClient` exposes `client.apiTokens.create`, `list`, `revoke`, and `revokeUser`; token clients cannot administer tokens. Self-service targets the connected user. A superuser may use `for_user` on create/list or `user` on revokeUser; the client builds administrative HMAC confirmation. `tokens.allow_self_service: false` disables own-token creation only; own-token list and revoke remain available.

Standard Node and browser WebSocket SCRAM use binding mode `0x02`; server refuses token creation from those sessions with `token_browser_session`. This binding protects the browser credential from export; it is not a general statement that all Node SCRAM issuance is denied. Node `connectLocal` uses binding mode `0x00` over local IPC and can issue tokens when authorized. IPC has no TLS: OS socket permissions or Windows DACL are the security boundary. `connectLocal` is absent from the browser entry.

```ts
import { connectLocal } from '@shamir/client';

declare const localEndpoint: string; // supplied by deployment configuration
const client = await connectLocal({
  addr: localEndpoint, username: 'operator', password: '<password>',
});
const own = await client.apiTokens.create({
  label: 'reports-worker', mode: 'read_only', channels: 'rest',
  databases: ['reports'], ttl_secs: 86_400,
});
// Keep and use this opaque handle; never log/extract its secret.
await client.apiTokens.list();
await client.apiTokens.revoke(own.id);
const delegated = await client.apiTokens.create({
  label: 'reports-reader', for_user: 'report-reader',
  mode: 'read_only', channels: 'rest', databases: ['reports'],
});
await client.apiTokens.list({ for_user: 'report-reader' });
await client.apiTokens.revokeUser({ user: 'report-reader' });
await client.close();
```

`ApiToken` redacts string/JSON/Node inspection and has no public reveal method. Use it directly with `connect`; the SDK cannot export a raw token string. `CreatedApiToken` contains `id` (22-character base64url), opaque `token`, `label`, `owner`, `mode`, `channels`, `databases`, `created_at_ns: bigint`, and `expires_at_ns: bigint`; its secret is returned once and never listed. `ApiTokenInfo` omits secret and adds `revoked_at_ns: bigint | null`. Names are snake_case. `isTokenAdminError(err)` narrows a `ShamirDbError` by code; `TOKEN_ADMIN_ERROR_CODES` lists the set, not the originating operation.

| Token-admin error | Meaning |
|---|---|
| `permission_denied` | Insufficient privilege. |
| `token_not_found` | Unknown/inaccessible token. |
| `token_browser_session` | Creation refused from browser-bound WS SCRAM. |
| `token_request_denied` | Forbidden admin operation on token session. |
| `token_self_service_disabled` | Self-service creation disabled. |
| `read_only_replica` | Online mutation unavailable on this node. |
| `hmac_required`, `hmac_mismatch` | Missing/invalid superuser confirmation. |
| `session_invalidated` | SCRAM session invalidated. |
| `invalid_username`, `not_found`, `not_supported`, `internal` | Target/input/compatibility/internal failure. |
| `token_label_exists`, `token_limit_reached`, `token_ttl_invalid`, `invalid_request` | Issuance policy or validation failure. |

## Connect with a token

`connect` takes `auth: 'token'`, an origin-root `url`, and a string token or `ApiToken`. Schemes are HTTP `http:`/`https:` and WS `ws:`/`wss:`. `channel` is inferred if omitted and must match the scheme if supplied. Token options do not accept SCRAM `username`, `password`, `host`, or `port`.

```ts
import { ApiToken, Batch, connect, Query } from '@shamir/client';

const client = await connect({
  auth: 'token', channel: 'http', url: 'https://db.example.test',
  token: ApiToken.from('<TOKEN_FROM_SECURE_INPUT>'),
});
// HTTP is lazy; the first request (ping here) authenticates.
await client.ping();
const reports = await Batch.create('recent-reports')
  .add('rows', Query.from('items').whereEq('status', 'ready'))
  .execute(client, 'reports');
await client.close();
```

```ts
import { ApiToken, connect } from '@shamir/client';

const client = await connect({
  auth: 'token', channel: 'ws', url: 'wss://db.example.test',
  token: ApiToken.from('<TOKEN_FROM_SECURE_INPUT>'),
});
await client.ping();
const rows = await client.db('reports').query('items')
  .whereEq('status', 'ready').rows();
await client.close();
```

```ts
import { ApiToken, connect } from '@shamir/client/browser';

const http = await connect({
  auth: 'token', channel: 'http', url: 'https://db.example.test',
  token: ApiToken.from('<TOKEN_FROM_SECURE_INPUT>'),
});
await http.ping();
await http.close();
const ws = await connect({
  auth: 'token', channel: 'ws', url: 'wss://db.example.test',
  token: ApiToken.from('<TOKEN_FROM_SECURE_INPUT>'),
});
await ws.ping();
await ws.close();
```

Examples assume provisioned `report-reader`, database `reports`, table `items`, and required rights. URLs must be explicit origin roots only: no endpoint path, userinfo, query, or fragment; HTTP channel uses `http:`/`https:` and WS uses `ws:`/`wss:`. For example, loopback HTTP `http://127.0.0.1:7340` pairs with WS `ws://127.0.0.1:7340`; use `https:`/`wss:` in production. Node `origin` is an optional WS-upgrade Origin value; browser cannot set it. Node TLS defaults to certificate verification; `tls.ca` accepts PEM contents. `tls.allowInsecureTlsForDevelopment: true` disables verification and is development-only, never production. Browser rejects Node `tls`/`origin` options and uses platform trust. Token `connectTimeoutMs` defaults to 10,000 ms and `requestTimeoutMs` to 35,000 ms; zero disables each. `signal` controls connection/request lifetime. HTTP connects lazily; WS resolves after binding.

### Scope and channel capabilities

Scope is fixed at issuance, never widened by `connect`: `mode` is `read_only` or `read_write`; `channels` is `rest`, `ws`, or `both`; `databases: null` means all databases accessible to the owner, while an explicit list narrows that access.

Effective rights intersect current owner ordinary data rights and token scope; tokens are not superusers or replicators. `Call` counts as a write. Read-only begin, reads, and rollback are allowed; `TxCommit` is refused for every read-only token, including transactions containing only reads. Consequently, SDK auto-commit `db.tx()` fails at commit for a read-only token. Use a `read_write` token for successful `db.tx()`.

| Capability | HTTP/REST | WebSocket |
|---|---|---|
| Ping, Execute/batch | Yes | Yes |
| Transactional Execute batch | Yes | Yes |
| Interactive transaction handle | No (`not_supported_over_rest`) | Yes, subject to scope |
| Visibility wait, DDL operation status | Yes | Yes |
| Cursor create/fetch/cancel | No (`not_supported_over_rest`) | Yes |
| Push subscriptions | No push channel | Yes (`runLive`) |
| DDL/access/token administration | Denied | Denied |

HTTP transactional batches are distinct from interactive multi-request transactions, which are WS-only. Token request allowlisting is deny-by-default. Only `list`, `access_tree`, `describe_table`, and `get_table_schema` are permitted introspection leaves, only top-level and subject to ordinary authorization. With an explicit database allowlist, requests that introspect databases/users (`List::Databases` and `List::Users`) are denied. Other list calls are not categorically denied; `AccessTree` must explicitly target an allowed database. Subscriptions must be top-level. The TS token path uses query version 4 and name-based builders; it does not use SCRAM interner-touch transport.

HTTP applies request allowlisting, framing/content type, timeouts, and `max_request_body_bytes` (default 16 MiB, server-configurable). Oversize requests may return 413. Independently, the SDK caps response bodies at `MAX_TOKEN_RESPONSE_BYTES` (default WS frame limit, 16 MiB). HTTP has no push.

Both token WS endpoints use `shamir-v1`. Native Node uses `/shamir/v1/ws` with Authorization bearer on upgrade. Browser uses `/shamir/v1/ws/browser`, then SDK sends the first binary token frame (max 256 bytes); server expects it within 5 seconds. Applications need not encode it. Browser Host must match `host_authority_allowlist`; Origin must match `browser_origin_allowlist`. Browser Host mismatch returns 400; Origin allowlist mismatch returns 403. Missing/malformed Host or Origin can also fail 400. Browser plaintext `ws:` is loopback-only. Native Origin policy is listener-configured. CORS headers are absent and not client-configurable. Active socket defaults are 8 per token and 32 per owner (configurable).

Token WS session deadline is the earlier of token expiry and `tokens.session_max_age_hours` (default 24 hours; configurable up to 24). Token WS has no idle timeout. `TokenWsClient.expiresAtNs` is the server session deadline in bigint nanoseconds, not token metadata. A `4012` close maps to `token_expired` for either deadline. Compare issuance metadata before re-login; if only session age elapsed and the token is still live, reconnect with that token.

## Password login over HTTP

`login(...)` is exported by both entry points. It proves password knowledge and returns a scoped bearer token after server-signature verification; it does not create a SCRAM database session. Login requires enabled server routes. Browser HTTP/login is same-origin: no CORS headers or client CORS option. A same-origin reverse proxy is an option; browser WS may use an exact allowed Origin, though same-origin is simpler.

```ts
import { login } from '@shamir/client';

declare const trustedListenerId: Uint8Array; // trusted 16-byte deployment value
const result = await login({
  username: 'report-reader', password: '<password>', url: 'https://db.example.test',
  scope: { label: 'reports-login', mode: 'read_only', channels: 'both', databases: ['reports'] },
  expectedKdf: { memoryKb: 131_072, time: 4, parallelism: 1 },
  listenerId: trustedListenerId,
  signal: new AbortController().signal,
});
const client = await result.connect({ channel: 'ws' });
await client.ping();
await client.close();
```

```ts
import { login } from '@shamir/client/browser';

declare const trustedListenerId: Uint8Array;
const result = await login({
  username: 'report-reader', password: '<password>', url: 'https://db.example.test',
  label: 'reports-login', mode: 'read_only', channels: 'both', databases: ['reports'],
  expectedKdf: { memoryKb: 131_072, time: 4, parallelism: 1 },
  listenerId: trustedListenerId,
});
const client = await result.connect({ channel: 'http' });
await client.ping();
await client.close();
```

Samples assume a unique active label and provisioned rights; collision returns `token_label_exists`, so choose a fresh label. `131072/4/1` matches `deploy/server.example.ktav`, not a universal default. Set `expectedKdf` to trusted server configuration; mismatch rejects. Server persists a generated listener ID if none configured. If pinning, supply trusted 16-byte `Uint8Array`. `expectedKdf` is an exact expectation, not a request to change server settings.

```ts
import { login } from '@shamir/client';

declare const developmentCaPem: string; // PEM certificate text, not a file path
const developmentLogin = await login({
  username: 'report-reader', password: '<password>', url: 'https://db.example.test',
  ca: developmentCaPem, timeoutMs: 15_000,
  scope: { label: 'development-login', channels: 'both', databases: ['reports'] },
});
const developmentClient = await developmentLogin.connect({ channel: 'http' });
await developmentClient.ping();
await developmentClient.close();
```

The CA PEM is retained for resulting HTTP and WS connections; the certificate must match the hostname. `timeoutMs` bounds the whole login ceremony, including crypto; abort may stop waiting but cannot interrupt active Argon2.

Login accepts top-level `label`, `mode`, `channels`, `databases` or nested `scope`, not both. Defaults: label `http-login`, mode `read_only`, channels `rest`, databases `null`; `ttl` is forbidden. Server `tokens.login_ttl` defaults to 86,400 seconds, capped at 24 hours. Login issuance is for eligible regular accounts without superuser/replicator flags; read-write requires `tokens.login_allow_read_write`. Tokens are not automatically refreshed.

`LoginResult.metadata.id` is a 16-byte `Uint8Array`; admin token `id` is a 22-character base64url string.

Database scope must contain 1–4 nonempty names, each 1–255 UTF-8 bytes, matching `[A-Za-z0-9_-]+`, NFC-normalized, unique, bytewise sorted; encoded scope <=512 bytes. Labels are 1–128 UTF-8 bytes and may not contain C0/DEL. Username canonicalization is partial UsernameCaseMapped: runtime Unicode tables are not pinned to 15.1 and right-to-left names are rejected; do not assume full PRECIS support.

HTTPS direct expects security class 0x01; HTTP is allowed only for loopback (0x00). The client does not infer proxy TLS: `externalTls: true` is only for an explicitly configured HTTPS-edge class 0x02; HTTP plus that flag is rejected. `ca` is Node-only PEM trust material, used during login and retained for the resulting client's HTTP/WS connection; it never disables verification. Browser uses platform trust. The SDK verifies the login transcript signature before returning a token. On `server_signature_mismatch`, it discards the token and rejects. This proves expected verifier-key possession, not server hostname/identity or application pinning.

`timeoutMs` defaults to 0: no total ceremony deadline. The development CA example explicitly sets a 15-second total deadline, including crypto. `connectTimeoutMs` defaults to 10,000 ms for the entire BEGIN exchange, not just socket open. `requestTimeoutMs` defaults to 120,000 ms for FINISH and is inherited by the resulting client's request timeout unless overridden. `signal` is not inherited by `result.connect()`. `LoginResult.metadata` uses camelCase `createdAtNs`/`expiresAtNs` bigint, unlike snake_case admin metadata.

The browser returns login tokens in `LoginResult`; keep them in memory only for the shortest practical lifetime, never logs or persistent browser storage. JavaScript strings cannot be reliably zeroized, so minimizing references is best effort. Browser-specific limits: same-origin HTTP/login (no CORS), platform TLS trust, no `ca`, `tls`, or `origin` options, and no setting WebSocket Origin; browser WS uses first-frame token flow with server Host/Origin allowlists. Plaintext `ws:` remains loopback-only. Do not persist tokens in `localStorage`; `sessionStorage` is an explicit risk exception, not XSS protection. Login lockout is per user+subnet pair: 50 failures lock that pair for one hour. `RetryAfter` does not distinguish the source or pair; a correct password cannot bypass the lock. An administrator may unlock the pair or it expires.

The client does not automatically retry login. Treat `rate_limited` (429) and `server_busy` (503) as application-level backoff signals: honor validated `retryAfterMs` when present and add jitter; bound retries and stop on abort, other errors, or exhausted policy. Do not retry writes after uncertain transport outcomes. Login tokens are not refreshed automatically. On `token_expired`, perform a new login using the current credential and then replace the in-memory token/client; do not assume an old token or interrupted login was revoked. Login `relogin` is true only for `LoginError('token_expired')`; later connection/operation expiry may instead be `TokenTransportError('token_expired')`. For WS session-age expiry, compare token expiry metadata: reconnect with the same token only if it remains unexpired; otherwise log in again.

### Login errors

| Code | Meaning / action |
|---|---|
| `invalid_request`, `protocol_error` | Invalid options or malformed protocol. |
| `authentication_failed` | Generic failure; do not reveal whether username exists. |
| `server_signature_mismatch` | SDK discards the token; this is not server identity pinning. |
| `rate_limited` | Login HTTP 429; header/body retry values must match. `retryAfterMs` is at least 1000 ms when supplied from the validated Retry-After seconds header (1–3600 seconds); no automatic retry. |
| `server_busy` | Login HTTP 503 with validated retry timing; back off. |
| `client_aborted`, `client_timeout`, `http_timeout`, `network_error` | Caller abort, timeout, HTTP or network failure. |
| `http_body_too_large`, `http_bad_headers`, `http_unsupported_media_type`, `http_method_not_allowed` | HTTP contract/framing error. |
| `unsupported_server` | Required login endpoint returned 404; declared `http_not_found` is not emitted for this case. |
| `token_expired` | Declared `LoginError` code; `relogin` is true only for this code. Continuation expiry is a `TokenTransportError`, not a promise of `LoginError` emission. |
| `kdf_params_rejected`, `security_mismatch` | Reject invalid or unexpected KDF/security/listener parameters. |
| `origin_forbidden`, `authority_forbidden`, `proxy_forbidden` | Listener Origin/Host/proxy policy denial. |
| `login_write_forbidden`, `token_self_service_disabled`, `login_read_only_replica`, `login_account_denied` | Issuance policy/account eligibility. |
| `token_label_exists`, `token_limit_reached` | Label collision/token cap. |

Login error reference includes every declared code. Login 404 maps to `unsupported_server`; `http_not_found` is declared but not emitted for the endpoint 404. `LoginError` exposes `code`, optional `status`, optional `retryAfterMs`, and `relogin`; it differs from later `TokenTransportError`. `LoginError.relogin` is true only when its code is `token_expired`; this does not promise a particular result-metadata expiry error path. After login, `result.connect()` or client operations may instead throw `TokenTransportError('token_expired')`. Signature mismatch is checked before token return; it proves expected verifier-key possession, not hostname/identity pinning. Authentication failures remain generic.

## HTTP status and transport errors

| HTTP | SDK code/content | Action |
|---|---|---|
| 200 | MessagePack `DbResponse`; engine error is still application error. | Inspect typed result; HTTP success does not mean query success. |
| 400 | Malformed request may map `http_bad_request`; unsupported REST/application code may be in body. | Fix shape/use WS as appropriate. |
| 401 | `unauthorized`, uniform invalid/unknown/expired/revoked credential. | Replace token; do not infer existence. |
| 403 | Scope/policy code (`token_read_only`, `token_database_denied`, `token_request_denied`, etc.). | Correct scope/policy. |
| 404 | `http_not_found` when route handler reached; auth may precede route. | Verify route/listener. |
| 405 | `http_method_not_allowed`. | Use supported method. |
| 408 | `http_timeout`. | Diagnose; retry only if safe. |
| 413 | `http_body_too_large`. | Reduce request/review cap. |
| 415 | `http_unsupported_media_type`. | Use SDK MessagePack. |
| 431 | `http_bad_headers`. | Reduce headers. |
| 429 | `rate_limited`; parses Retry-After to `retryAfterMs` when supplied. | Back off; no automatic retry. |
| 503 | `server_unavailable`; parses Retry-After when supplied. | Bounded backoff after recovery. |

The SDK caps HTTP response bodies at `MAX_TOKEN_RESPONSE_BYTES` (default WS frame limit, 16 MiB); server request `max_request_body_bytes` is separate. Token failure limiting is separate from SCRAM lockout. Missing Authorization does not debit the failure quota; valid credentials are never rejected by that failure limiter. Separate post-auth request rate limits still apply. Login uses separate proof/source limits. Honor retry timing; do not tight-loop.

`TokenTransportErrorCode` union: `token_read_only`, `token_database_denied`, `token_request_denied`, `token_channel_denied`, `token_plaintext_write_denied`, `not_supported_over_rest`, `invalid_request`, `token_label_exists`, `token_limit_reached`, `token_ttl_invalid`, `token_not_found`, `token_self_service_disabled`, `token_browser_session`, `permission_denied`, `hmac_required`, `hmac_mismatch`, `http_bad_request`, `unauthorized`, `http_not_found`, `http_method_not_allowed`, `network_error`, `client_timeout`, `http_timeout`, `http_body_too_large`, `http_unsupported_media_type`, `http_bad_headers`, `rate_limited`, `server_unavailable`, `protocol_error`, `client_aborted`, `tls_configuration_error`, `connection_closed`, `authentication_failed`, `token_sid_mismatch`, `token_revoked`, `token_expired`, `token_channel_unavailable`, `first_frame_timeout`, `preauth_overloaded`, `server_busy`, `timeout`, `lock_timeout`, `tx_conflict`, `read_only_replica`, `wal_retention_exceeded`. Properties: `status`, `retryable`, `retryAfterMs`, `phase`; errors are sanitized. `ShamirDbError` represents decoded DB/application errors, including engine failures returned in an HTTP 200 response; these are not transport errors. `TokenTransportError` covers transport and token-policy failures, not every server application error. Writes are not automatically retried because a lost response can leave the outcome uncertain.

## WebSocket close codes and recovery

| Close code | Meaning / action |
|---|---|
| `4000` | Browser pre-binding authentication failure; surface generic failure. |
| `4010 token_sid_mismatch` | Session/SID mismatch; investigate client/protocol state. |
| `4011 token_revoked` | Token revoked/invalidated; obtain replacement. |
| `4012 token_expired` | Token or session deadline ended. SDK maps both to `token_expired`; `expiresAtNs` is session deadline. Reconnect with the same still-unexpired token if only session max-age elapsed. |
| `4013 token_channel_denied` | Verified channel denial; correct scope/use allowed channel. Browser pre-binding may collapse code. |
| `4014 token_plaintext_write_denied` | Plaintext listener policy; use TLS or read-only scope. |
| `1013 server_busy` | Temporary capacity; bounded backoff. |
| `1000` | Normal closure/shutdown. |
| `1002`, `1003`, `1009` | Protocol/text/oversize frame; correct client behavior. |
| `4003` | Heartbeat/network failure; treat as lost session. |

Server WS checks token validity before request dispatch and tears down transactions/cursors/subscriptions on revoke/expiry. An in-flight write outcome may be uncertain; do not blindly replay. No auto reconnect/resumption; each new WS session has a new SID and cursors/subscriptions must be recreated. Node may map upgrade 403 to `permission_denied` without Retry-After. Browser pre-binding failures including `4013`, `4014`, `1013` may collapse to `authentication_failed`; do not promise distinct errors. After binding, pending requests map close codes; future calls generally fail `connection_closed`; iterators may finish and no general error-event API is exposed.

## Security guidance

- Keep tokens in memory. Avoid `localStorage` unless an explicit policy exception accepts same-origin script exposure. `sessionStorage` is only a deliberate short-lived persistence exception, not XSS protection.
- Never log, print, put in telemetry/URLs, or commit tokens. `ApiToken` redacts common conversions but cannot protect explicitly handled raw strings.
- Prefer TLS 1.3 and certificate verification remotely. SDK has no application identity pin/channel binding; plaintext bearer traffic is visible to observers.
- Rotate by issuing replacement, switching consumers, validating, then revoking old token. Revoke after exposure. Password/role/privilege changes, user deletion, restore can invalidate tokens.
- CORS is absent and not client-configurable. Host/Origin allowlists, proxy trust, login policy and caps belong to listener config. Token listener does not host UI; host separately or use same-origin reverse proxy.
- Use restrictive application CSP with `connect-src` limited to approved origins. Do not infer WASM/CSP guarantees from specs; verify actual application requirements.
- Honor 429 Retry-After with backoff/jitter. Bearer auth and login proof/source limiters are distinct; never tight-loop.

## Source references and caveats

[Operator listener settings](../../../docs/guide-docs/guide/07-operations.md#api-token-listener-operations) · [Token API](../../../docs/guide-docs/client-server-protocol-spec/TOKEN_API.md) · [HTTP login](../../../docs/guide-docs/client-server-protocol-spec/TOKEN_LOGIN_HTTP.md)

Public API: `crates/shamir-client-ts/src/index.ts`, `src/browser.ts`, `src/core/types/connection.ts`, `src/login/types.ts`, `src/login/errors.ts`. Admin/opaque token: `src/core/types/access/token-admin.ts`, `src/core/support/errors.ts`, `src/entry/api-token.ts`, `src/core/client/client.ts`, `src/core/client/client-internals.ts`. Transport: `src/entry/token-client.ts`, `src/entry/token-db.ts`, `src/entry/ws/token-client.ts`, `src/entry/ws/token-lifecycle.ts`, `src/core/protocol/token/`, `src/platform/connect/node-connect.ts`, `src/platform/connect/browser-connect.ts`. Login: `src/login/core.ts`, `src/login/protocol.ts`, `src/login/node/node-http.ts`, `src/login/browser/browser-http.ts`. CLI: `crates/shamir-server/src/security/tokens/cli/args.rs`, `create.rs`, `list.rs`, `revoke.rs`, `secret_output.rs`. Operator reference: `docs/guide-docs/guide/07-operations.md`.

Existing source-reviewed usage includes `crates/shamir-client-ts/src/__tests__/connection/token/e2e-login.test.ts`, browser/revocation/channels tests, `src/entry/__tests__/`, `src/entry/ws/__tests__/`, `src/login/__tests__/`, and `src/core/client/__tests__/api-token-admin.test.ts`. These sources were reviewed, not executed; no runtime tests/builds/CI were run.
