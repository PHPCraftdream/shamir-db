# Token API operations guide

This guide describes the server's implemented token API for operators configuring listeners, issuing credentials, monitoring access, and recovering service. Code references below are repository-relative; implementation is authoritative where protocol documents differ.

## Listener exposure and TLS

Token REST and password login are mounted on a `kind: http` listener. Set `tls` explicitly. Plaintext is permitted on loopback; a remote plaintext bind requires `allow_plaintext_remote: true`. Remote plaintext read-write token use additionally requires `allow_plaintext_write: true`; that flag is invalid without remote-plaintext opt-in. Both flags are invalid with TLS. Avoid bearer credentials over plaintext networks.

TLS requires the server certificate/key (`tls.cert_path`, `tls.key_path`); TLS 1.3 transport policy applies. `allow_self_signed` is a startup refusal override for a self-issued certificate on a non-loopback HTTP TLS listener; it does not alter peer-certificate verification. Prefer a publicly trusted certificate. `kind: http` must not set `profile`.

Direct TLS listener example (replace host and certificate paths for deployment):

```ktav
{
    kind: http
    addr: 0.0.0.0:443
    tls: true
    host_authority_allowlist: [db.example.test]
    browser_origin_allowlist: [https://db.example.test]
    trusted_proxies: []
    max_request_body_bytes: 16777216
    body_timeout_ms: 30000
    request_timeout_ms: 120000
    login: {
        enabled: true
        authority_allowlist: [db.example.test]
    }
}
```

`host_authority_allowlist` and `browser_origin_allowlist` are distinct exact allowlists; the latter is not wildcard or CORS configuration. The TLS fragment above is ONE `listeners[]` entry: put it inside the existing `listeners` array and supply top-level `tls`. For defaults, TLS-related plaintext flags are false, `trusted_proxies`, `host_authority_allowlist`, and `browser_origin_allowlist` are empty (closed browser access).

### Proxy trust

For reverse-proxy TLS termination, set `external_tls: true` only for a loopback backend, set non-empty trusted proxy CIDRs, and explicitly configure login authorities and canonical HTTPS browser origins. TLS plus `external_tls` is invalid; do not let untrusted peers reach the backend. Login is not enabled for remote plaintext. Direct remote TLS login requires explicit `login.authority_allowlist`; login also forbids listener `kdf_override`.

### Certificate startup

TLS startup requires certificate/key paths. For remote self-issued certificates, startup classifies a certificate as self-issued when the encoded issuer equals the subject; non-loopback listeners refuse it unless `allow_self_signed` is enabled. The override emits an SPKI SHA-256 warning. TLS 1.3 negotiates `http/1.1`; there is no 0-RTT or mutual TLS. `load_or_generate` creates a certificate if both certificate and key are missing, then remote startup can refuse it as self-issued. Client certificates are not used for authentication. Sources: `crates/shamir-server/src/engine/server/server_launcher/http/tls_policy.rs:20-49`, `crates/shamir-server/src/boot/config/http_listener.rs:85-262`.

### Resource defaults

Listener budget defaults: request body 16 MiB (`max_request_body_bytes`), body timeout 30,000 ms, request timeout 120,000 ms; head timeout inherits `security.connection.auth_init_timeout_ms` (5,000 ms default). Values must be positive; head/body timeout cannot exceed request timeout. Login requires effective head timeout no more than 5,000 ms. Login BEGIN and FINISH caps are 4,096 and 256 bytes, respectively. Keep these separate limits appropriate to workload. HTTP server transport head bounds are 8,192 bytes and 64 headers **per request**, not a process-wide aggregate.

The shared HTTP body budget is 16 times the largest configured HTTP listener `max_request_body_bytes` (`crates/shamir-server/src/engine/server/server_launcher/listeners.rs:178-187`): default 256 MiB (268,435,456 bytes); if the small preset's 4 MiB cap is largest, 64 MiB. Configure `security.connection.max_active_connections` (default 1,000) and `max_active_connections_per_ip` (default 100); zero disables each limit. Only trusted TCP peers are exempted, and only when login is enabled AND external_tls=true; global cap still applies (`crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:204-218`).

The global query response budget defaults to 256 MiB (268,435,456 bytes) and per-result cap to 64 MiB (67,108,864 bytes). `security.query_limits.max_inflight_response_bytes` may be null (unbounded; warn operators), and otherwise must be at least `max_result_size_bytes`. These are server-wide settings, not endpoint-specific knobs.

## Token policy, scope and channels

Token configuration defaults: `default_ttl_days: 30`, `max_ttl_days: 365`, `max_per_user: 20`, `allow_self_service: true`, `session_max_age_hours: 24`, `max_sockets_per_token: 8`, `max_sockets_per_owner: 32`, `login_ttl: 86400`, `login_allow_read_write: false`. Configured TTLs must be positive. The maximum allowed configured TTL is `max_ttl_days`; `default_ttl_days` must be less than or equal to `max_ttl_days`; labels must be unique among active tokens. Active-token counts include only unexpired, unrevoked tokens. Limits include 365-day maximum TTL, 1,000 tokens per user, 24-hour maximum session age, 1,024 sockets per token and 4,096 per owner; configured caps must be positive and owner cap must cover per-token cap. Login TTL is 1–86,400 seconds. Token endpoints are always both enabled; there is no endpoint-selection setting.

Scopes include read-only or read-write mode. `databases=None/null` means every database the ordinary owner can access; `Some([])` allows none; explicit names narrow the owner's rights. Channels are REST, WS or Both; CLI values are `http`, `ws`, `both` (Both by default). Choose the narrowest scope, database set, channel and lifetime. Offline token creation defaults to read-only. HTTP login issues read-write scopes only when `login_allow_read_write` is enabled. Tokens are node-local and not replicated; issue and revoke them on each relevant node.

## CLI issuance and online administration

CLI token maintenance uses `shamir-server --config <config> token ...` and requires the server stopped so its maintenance lock is available. Use a TTY for secret output or a private `--out-file`; protect and delete output securely. TTL syntax accepts duration values such as `30d`, `1h`, and `60s`, not only whole days. Example:

```text
shamir-server --config <config> token create --user <user> --label <label> --ttl 30d --channels both --out-file <private-file>
shamir-server --config <config> token list [--user <user>]
shamir-server --config <config> token revoke <id>
shamir-server --config <config> token revoke-user <user>
shamir-server --config <config> token revoke-all --yes
```

Creation prints the bearer secret once; list shows metadata only. `revoke-all --yes` revokes tokens but does not rotate the token HMAC key. There is no supported key-rotation process in v1; on incident, revoke/reissue credentials and address compromised metadata or host through isolation and recovery procedures. `SECURITY_MODEL.md:96` says the key rotates, which diverges from the implemented CLI behavior; do not rely on that statement.

Online administration exposes exactly `CreateApiToken`, `ListApiTokens`, `RevokeApiToken`, and `RevokeUserTokens`; use the typed builder functions `shamir_query_builder::wire::token_admin::{create_api_token,list_api_tokens,revoke_api_token,revoke_user_tokens}`; see the [TypeScript token guide](../../../crates/shamir-client-ts/docs/TOKENS.md). These operations are SCRAM-only. Self list/revoke remains available even when `allow_self_service=false`; that setting blocks self creation. Browser-bound creation is specifically denied as `token_browser_session`, not a blanket ban on all browser admin operations. `for_user` requires a superuser and valid HMAC authorization, even when explicitly naming the caller's own user. Ordinary owner roles are narrower than superuser/replicator; `read_only` denies `TxCommit`, and `Call` counts as a write. Owner permissions are constructed with `SessionPermissions::new(false,false,owner_roles)`, which removes superuser and replicator flags; roles are a snapshot. Effective rights are the intersection of owner rights and token scope, without admin, DDL, token-admin, password, or replication authority. Scoped tokens are always denied top-level `ListDatabases` and `ListUsers` (`crates/shamir-server/src/security/perimeter/http_rest/scope.rs:157-164`); `AccessTree` allows explicitly allowlisted databases and denies missing/outside databases (165-178). T7's interpretation of global list behavior remains BLOCKED SPEC. Offline maintenance can operate on a read-only replica; online mutations cannot. CLI supports `--read-write`, repeated `--db`, and `--channels http|ws|both` (not endpoint selection); new output files use create-new semantics, Unix mode 0600, and Windows ACL warning. Sources: `crates/shamir-server/src/engine/db_handler/handlers/token_admin/logic.rs:22-111`, `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs:651-698`, `crates/shamir-server/src/security/tokens/cli/`.

HTTP password login follows its challenge/finish flow and returns a scoped bearer. Protect credentials, response bodies, browser storage and bearer output. A lost response after successful issuance can leave the token active until expiry or revocation.

## HTTP login policy

`login.enabled` defaults to `false`; `login.authority_allowlist` defaults to `[]` (loopback aliases). Optional `listener_id` is an explicit 32-lowercase-hex identifier; if omitted, a random 16-byte identifier is persisted by configured address. An explicitly configured ID is not automatically persisted. `external_tls` defaults to `false`. Direct plaintext login is loopback bind and peer only. With external TLS, use a loopback backend, trusted proxy, explicit authority, and HTTPS origins; remote plaintext login is not allowed. Login uses fresh SCRAM proof, not password POST; the client verifies the server signature before accepting a token. Privileged superuser/replicator accounts are denied with `login_account_denied`; `allow_self_service: false` blocks self-service creation. Read-only replica login returns 403 `login_read_only_replica`. `login_allow_read_write` defaults false. Effective TTL is `min(login_ttl, max_ttl_days * 86400)`, with `login_ttl` in 1..86400 and default 86400. `security.login_enumeration_hardening` defaults true and requires the stored KDF tuple to equal defaults and client limits; startup telemetry is a count gauge only. Sources: `crates/shamir-server/src/boot/config/http_listener.rs:157-217`, `crates/shamir-server/src/boot/config/login_listener.rs:8-18`, `crates/shamir-server/src/security/perimeter/http_login/service/finish.rs:125-151`, `crates/shamir-server/src/security/perimeter/http_login/service/issuance.rs:98-103`, `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:327-350`.

## Key persistence, backup and restore

The server generates and persists a 32-byte HMAC key in server metadata on first initialization; reopen reuses it and malformed stored length fails. It is not a user-editable `token_key` configuration key. Back up the complete data directory and metadata together, offline after a clean stop. A restore advances each owner's shared `tickets_invalid_before_ns` floor; a token is valid only when its `created_at_ns >` owner's shared `tickets_invalid_before_ns` floor. Password changes, role/flag changes, explicit user-ticket revocation, and user deletion invalidate tokens; deletion removes the owner's cache. `TOKEN_API§5` documents `kick_session` as not affecting API tokens; do not use kicks for token revocation. The older `connect::ops::admin::kick_session` helper instead bumps the ticket floor; it has no production callsite found. This discrepancy is tracked in the T8 report; use explicit token revoke. Sources: `crates/shamir-server/src/security/tokens/service/token_service.rs:544-557`, `crates/shamir-server/src/sync/durability/restore.rs:394-398`, `docs/guide-docs/KNOWN_LIMITATIONS.md` (Token API section). Reissue credentials after restore as needed. `revoke-all` alone is not key rotation. Treat backups as credentials and never hand-edit metadata.

## Browser and transport choices

Token REST endpoints are `POST /shamir/v1/request` and `GET /shamir/v1/ping`; native WS is `GET /shamir/v1/ws` with `Authorization`, Browser WS is `GET /shamir/v1/ws/browser`, then the first binary frame carries only the token; server sends binding SID after verification; this is a narrowly scoped first-frame credential exception, not a general header-only rule. Login is `POST /shamir/v1/auth/begin` and `/finish`; `login.enabled` defaults false. The REST wire protocol is MessagePack only: one `DbRequest` yields one `DbResponse`; JSON is unsupported. Requests require exactly one `Authorization: Bearer` credential; cookies and query-string credentials are not accepted. Wrong, expired and revoked bearer credentials receive uniform 401 responses. Invalid authentication may receive 429 from the limiter; valid tokens are not blocked by that limiter, and requests without an Authorization header are not charged to it.

Authentication happens before body polling; authorization is revalidated after body receipt, and `Expect: 100-continue` is answered only after admission. Responses set no-store, nosniff and HSTS on direct TLS; there is no CORS. TLS clients MUST validate server chain and hostname. If exactly one certificate/key file exists, startup refuses; if both are absent, a self-issued pair is generated, subject to the remote HTTP refusal policy above. Token storage is under `data_dir/api_tokens`, alongside server metadata/users backup requirements. Revocation is durable before publication and then cancellation is attempted; existing authorized work may finish, so no exact-once cancellation guarantee is made. Sources: `crates/shamir-transport-http/src/server/service.rs:84-136,161-175,178-318`, `crates/shamir-transport-http/src/server/response.rs:28-45`, `crates/shamir-server/src/security/tokens/store.rs:54-56`. Credentials are `shk1.<id>.<secret>` (71 characters), canonicalized and shown once, sent in Authorization and never query parameters/cookies.

REST rejects interactive multi-request transactions (TxBegin/TxExecute/TxCommit/TxRollback), cursors, and subscriptions with not_supported_over_rest. Single-request transactional Execute batches remain supported; use token WebSocket for session-scoped operations. Unknown ordinary paths authenticate first, then return 404. Browser Host policy uses raw `Host`, not `X-Forwarded-Host`; trusted proxies affect XFF. For REST/WS failure attribution, XFF is walked right-to-left to the first untrusted address; per-IP limits use the TCP peer except for the trusted-proxy exemption above. Host authority canonicalization uses the backend listener's TLS setting (default port 80 or 443); Origin has its own http/https scheme and is separately canonicalized/allowlisted. No CORS headers are currently provided; same-origin access is an implementation possibility, not a verified runtime guarantee. See `crates/shamir-server/src/security/perimeter/http_rest/`, `crates/shamir-server/src/security/perimeter/http_ws/`, `crates/shamir-server/src/security/perimeter/http_login/`, and `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs`.

### WebSocket lifecycle

Subprotocol is `shamir-v1`. Native bearer authentication and wrong-channel rejection happen before HTTP 101; wrong channel returns 403. Browser WebSocket checks Host and Origin before 101, then requires a first binary credential frame within 5 seconds, maximum 256 bytes; the server binding SID is issued only after verification (it is not client binding). Pre-auth caps are fixed at 256 globally and 8 per subnet. Active caps are configurable, default 8 per token and 32 per owner, separate from the SCRAM limit of 16. Expiry is the minimum of token expiry and connection-open time plus `session_max_age_hours`. After binding, clients must use the server's connection-specific SID; another SID closes with 4010. Session lifetime has no idle timeout, only token expiry/max age and heartbeat. Token state is checked before each request; revocation closes with 4011 and cleans subscription/transaction cursors. Already-authorized work may finish. Browser WebSocket plaintext is loopback-only. Browser policy uses raw `Host`, not `X-Forwarded-Host`; Host authority canonicalization uses the backend listener's TLS setting (default port 80 or 443). Origin has its own http/https scheme and is separately canonicalized/allowlisted; `external_tls` for login does not change WebSocket backend TLS. Sources: `crates/shamir-server/src/security/perimeter/http_ws/route.rs:90-150,159-267`, `crates/shamir-server/src/security/perimeter/http_ws/browser/pre_auth.rs:10-11`, `crates/shamir-server/src/security/perimeter/http_ws/browser/first_frame.rs`.

Close codes: **1000** shutdown; **1003** text frame; **1009** oversized; **4003** heartbeat; existing **4010-4014** session/auth/policy reasons; **1013** busy.

## Shutdown and capacity

Shutdown ordering is transport connection drain (5 seconds), then login workers (min(`request_timeout_ms`, 7 seconds)), then audit drain (2 seconds). The outer listener deadline is 10 seconds, so these budgets can sum beyond it and be cut short; there is no guarantee all phases finish. See `crates/shamir-server/src/boot/config/http_mapping.rs:30`, `crates/shamir-transport-http/src/server/accept.rs:368-398`, `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:432-445`, `crates/shamir-server/src/engine/server/server_handle.rs:192-218` and [Known limitations](../KNOWN_LIMITATIONS.md). Response-write permit behavior remains unresolved; blocking-thread redaction/full-memory proof is also unresolved [T7 coverage audit](../../dev-artifacts/audits/2026-11-03-token-t7/DESIGN.md).

## Metrics, audit and logs

Token and login series are exported by the separate observability listener (`observability.addr`, default `127.0.0.1:9090`, `GET /metrics`), not the token listener; protect metrics access.

Metrics are `shamir_token_auth_total{channel=rest|ws,result=success|invalid|forbidden|rate_limited|unavailable}`, `shamir_token_request_denied_total{endpoint=rest|ws}`, `shamir_token_active_ws`, `shamir_token_listener_plaintext_remote` (set to 1 only for remote plaintext), and `shamir_token_request_duration_seconds{endpoint}`. `shamir_login_http_pair_slots` is a gauge; `shamir_login_http_accounting_dropped` includes `pair_quota` and `pair_table_full` from pair-store accounting (`crates/shamir-server/src/engine/server/server_launcher/launch.rs:276-285`), as well as admission `arrival_table_full` and `credit_table_full` (`crates/shamir-server/src/security/perimeter/http_login/metrics.rs:56-57`). Login metrics in `crates/shamir-server/src/security/perimeter/http_login/metrics.rs` are `shamir_login_http_total{endpoint,result}`, `shamir_login_http_pending`, `shamir_login_http_evictions{reason}`, `shamir_login_http_accounting_dropped{reason}`, `shamir_login_http_kdf_ineligible_users`, `shamir_login_http_begin_duration_seconds`, `shamir_login_http_finish_duration_seconds`; audit drop accounting is in `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:11-22,76`. Audit drops increment `shamir_login_http_audit_dropped`. Alert on sustained `invalid`/`rate_limited`, rising 503/body-pool rejection, increasing latency, active WebSockets near budget, any audit drop, and any compensation-critical event. Do not use per-token labels. Auth counters count authenticate outcomes, not every successful request. Audit queue drops may lose telemetry, and `api_token_exposed` should trigger credential revocation. `http_login_compensation_failed` is critical. There are no user/token labels. Log sanitization is path-bounded and supported by T7 source tests, not a blanket runtime guarantee. 

Audit events: `api_token_created` and `api_token_revoked` and `api_token_user_revoked` (`crates/shamir-server/src/security/tokens/service/token_service.rs`); `api_token_auth_failures` and `api_token_exposed` (`crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs:651-698`); `http_login_issued`, `http_login_failures`, `http_login_compensation_failed` (`crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:186-215`). Token IDs are not the secret or MAC. Audit aggregation/sampling may occur; append is best-effort and dropped events are possible, so there is no blanket complete/durable audit guarantee. Never log Authorization headers, bearer values, raw request/response bodies, login credentials, token output or proxy secrets. Redaction is path-specific; blocking-thread and downstream logging coverage is not established.

## Troubleshooting

| HTTP status / symptom | Meaning and operator action |
|---|---|
| 200 | Endpoint completed; engine-level errors may remain represented in a 200 response. |
| 400 | Includes `invalid_request` and `not_supported_over_rest`; validate request and endpoint. |
| 401 | REST bearer authentication failure uses `WWW-Authenticate: Bearer`; login authentication failure is `authentication_failed`. |
| 403 | Bearer scope/policy denial: `token_read_only`, `token_database_denied`, `token_request_denied`, `token_channel_denied`, `token_plaintext_write_denied`; login policy denials include `login_read_only_replica`/`login_account_denied`/`login_write_forbidden`. Online SCRAM admin `read_only_replica` remains a DbResponse error, not HTTP status. |
| 404 | Unknown ordinary path after authentication; check endpoint. |
| 405 | Unsupported method; use the endpoint's allowed method. |
| 408 | Head/body/request timeout. |
| 413 | Request body exceeds configured cap; 429 is rate limiting, while 503 includes request-body budget rejection/unavailability. |
| 415 | Unsupported content type; use `application/msgpack`. |
| 429 | Rate limit; reduce pressure and inspect metrics. |
| 431 | Headers exceed per-request transport bounds (8,192 bytes/64 headers). |
| 503 | Service unavailable or body-budget rejection; inspect body-pool pressure and distinguish from login replica denial. |
| `kind=http requires tls` | Set explicit `tls: true` or `false`. |
| `allow_plaintext_write requires allow_plaintext_remote` | Avoid plaintext writes or set both only for isolated intentional deployment. |
| `login cannot be enabled with allow_plaintext_remote` | Disable remote plaintext login; use TLS or a correctly configured proxy. |
| `external_tls requires loopback bind and non-empty trusted_proxies` | Bind backend to loopback and list only trusted proxy CIDRs. |
| `profile must be absent for kind=http` | Remove listener profile. |
| `HTTP head/body/request timeout must be greater than zero` | Set all effective timeout values greater than zero. |
| `head and body must not exceed request` | Increase request timeout or reduce head/body timeout. |
| `remote HTTP TLS listener refuses self-issued certificate; set allow_self_signed` | Use a non-self-issued certificate or explicitly enable the override. |
| `login_ttl must be in 1..=86400 seconds` | Configure login TTL within the supported range. |
| `token configuration exceeds sane caps` | Reduce token limits to the supported caps. |
| Authority/origin validation error | Use canonical authorities, HTTPS origins, and explicit remote/proxy values. |

WebSocket close codes: **4000** invalid browser credentials or framing; **1013** busy; **4010** session-ID mismatch; **4011** token revoked/owner invalidated; **4012** expiry/session age; **4013** browser channel denied; **4014** plaintext write denied. `not_supported_over_rest`/`invalid_request` map to HTTP 400. Configuration and TLS startup refusals are defined in `crates/shamir-server/src/boot/config/http_listener.rs:85-262`, token config, and `crates/shamir-server/src/engine/server/server_launcher/http/tls_policy.rs:20-49`.

## Source references

Listener settings and validation: `crates/shamir-server/src/boot/config/http_listener.rs`, `crates/shamir-server/src/boot/config/http_validation.rs`, `crates/shamir-server/src/engine/server/server_launcher/http/tls_policy.rs`, `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs`. Token policy and service: `crates/shamir-server/src/boot/config/tokens.rs`, `crates/shamir-server/src/security/tokens/service/token_service.rs`, token CLI under `crates/shamir-server/src/security/tokens/cli/`.

REST/WS/login: `crates/shamir-server/src/security/perimeter/http_rest/`, `crates/shamir-server/src/security/perimeter/http_ws/`, `crates/shamir-server/src/security/perimeter/http_login/`; HTTP transport budgets: `crates/shamir-transport-http/`.

Restore: `crates/shamir-server/src/sync/durability/restore.rs`; WebSocket close mapping: `crates/shamir-server/src/sync/framer.rs` and `crates/shamir-server/src/security/perimeter/http_ws/`. Persistent key: `crates/shamir-server/src/engine/server_meta.rs`.
