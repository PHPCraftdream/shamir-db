# TOKEN-JS0 — token client design for `shamir-client-ts`

**Status: design of stages JS1–JS5.** Normative amendments have been applied; this document retains client API, architecture, packaging and implementation-stage design, with normative protocol details left to the specifications.

## Owner decisions / implementation candidates

The protocol decisions below are accepted in the normative specifications. Remaining client design candidates: Node 20+ requires confirmation against supported CI/runtime policy; no cross-origin CORS support is proposed, and same-origin proxy is recommended. Host/proxy authority rules are specified in TOKEN_API §10.

## 1. Public API

**Observed.** `crates/shamir-client-ts/src/core/types/connection.ts:50-74` defines current connection options; Node `src/index.ts:16-22` and browser `src/browser.ts:12-18` expose SCRAM connect. Current client methods include `ping`, `execute`, `waitForVisibility`, and `getDdlOpStatus` (`src/core/client/client.ts:620-622,637,807-810,959-979`); result types are in `core/types/write/batch.ts:364-392` and `core/visibility/types.ts:3-9`. TOKEN_API §7 is the normative REST allowlist. Existing SCRAM options and overloads remain source-compatible.

**Decision.** Token options discriminate `auth: 'token'`, `channel: 'http' | 'ws'`; URL/channel mismatch is rejected without fallback. HTTP connect is lazy; HTTP deadlines apply per request. WS connect waits for upgrade and verified binding; `connectTimeoutMs` bounds upgrade/authentication. Request default is 35,000 ms; zero disables the corresponding timeout. `signal` controls the client lifetime. Require absolute origin-root URL without userinfo/query/fragment and path except empty or `/`. Derive `/shamir/v1/ping`, `/shamir/v1/request`, native `/shamir/v1/ws`, browser `/shamir/v1/ws/browser`; never append to caller path. HTTP accepts http/https; WS accepts ws/wss. Browser plaintext `ws://` is loopback-only. Node may specify per-client CA and optional Origin; browser cannot specify TLS or Origin.

```ts
export interface ConnectOptions { host:string; port:number; username:string; password:string; tls?:{rejectUnauthorized?:boolean}; origin?:string; requestTimeoutMs?:number; connectTimeoutMs?:number; }
export type ScramConnectOptions = ConnectOptions;
export interface TokenHttpConnectOptions { auth:'token'; channel:'http'; url:string; token:ApiToken; requestTimeoutMs?:number; connectTimeoutMs?:number; signal?:AbortSignal; }
export interface TokenWsConnectOptions { auth:'token'; channel:'ws'; url:string; token:ApiToken; requestTimeoutMs?:number; connectTimeoutMs?:number; signal?:AbortSignal; }
export type NodeTokenHttpConnectOptions = TokenHttpConnectOptions & { tls?:TokenTlsOptions; origin?:string };
export type NodeTokenWsConnectOptions = TokenWsConnectOptions & { tls?:TokenTlsOptions; origin?:string };
export type BrowserTokenHttpConnectOptions = TokenHttpConnectOptions & { tls?:never; origin?:never };
export type BrowserTokenWsConnectOptions = TokenWsConnectOptions & { tls?:never; origin?:never };
export type NodeConnectOptions = ScramConnectOptions | NodeTokenHttpConnectOptions | NodeTokenWsConnectOptions;
export type BrowserConnectOptions = ScramConnectOptions | BrowserTokenHttpConnectOptions | BrowserTokenWsConnectOptions;
export interface TokenTlsOptions { ca?:string|Uint8Array; allowInsecureTlsForDevelopment?:true; }
export function connect(opts:ScramConnectOptions):Promise<ShamirClient>;
export function connect(opts:NodeTokenHttpConnectOptions):Promise<TokenHttpClient>;
export function connect(opts:NodeTokenWsConnectOptions):Promise<TokenWsClient>;
export function connect(opts:NodeConnectOptions):Promise<ShamirClient|TokenHttpClient|TokenWsClient>;
// Browser entry preserves SCRAM and uses BrowserToken* overloads.
export function connect(opts:ScramConnectOptions):Promise<ShamirClient>;
export function connect(opts:BrowserTokenHttpConnectOptions):Promise<TokenHttpClient>;
export function connect(opts:BrowserTokenWsConnectOptions):Promise<TokenWsClient>;
export function connect(opts:BrowserConnectOptions):Promise<ShamirClient|TokenHttpClient|TokenWsClient>;
```

```ts
export class ApiToken {
  private constructor();
  static from(value: string): ApiToken;
  toString(): string;
  toJSON(): string;
  toLocaleString(): string;
  [Symbol.toPrimitive](hint: string): string;
}
export interface TokenHttpClient {
  readonly channel:'http'; readonly sid:undefined; readonly expiresAtNs:undefined;
  ping():Promise<{kind:'pong'}>;
  execute(db:string,batch:object,visibilityWait?:VisibilityWait):Promise<BatchResponse>;
  executeWithTouch(db:string,batch:object):Promise<BatchResponse>;
  waitForVisibility(db:string,repo:string,version:bigint,wait:VisibilityWait):Promise<VisibilityState>;
  getDdlOpStatus(db:string,repo:string,table:string,opId:string):Promise<DdlOpStatus|null>;
  db(name:string):TokenHttpDb; close():Promise<void>;
}
export type TokenWsClient = Omit<TokenHttpClient,'channel'|'sid'|'expiresAtNs'|'db'> & {
  readonly channel:'ws'; readonly sid:Uint8Array; readonly expiresAtNs:bigint;
  db(name:string):TokenWsDb;
};
export interface TokenHttpDb {
  readonly name:string; run(op:object):Promise<QueryResult>;
  rows(op:object):Promise<Array<Record<string,WireValue>>>;
  query(table:string):Query; withRepo(repo:string,table:string):Query;
  batch(id?:WireValue):Batch; execute(batch:BatchRequest):Promise<BatchResponse>;
  waitForVisibility(repo:string,version:bigint,wait:VisibilityWait):Promise<VisibilityState>;
  getDdlOpStatus(repo:string,table:string,opId:string):Promise<DdlOpStatus|null>;
}
export interface TokenWsDb extends TokenHttpDb {
  cursor(query:Query|ReadQuery,pageSize:number):CursorIterator;
  tx<T>(fn:(tx:TokenTx)=>Promise<T>,opts?:TransactionOptions):Promise<TransactionResult<T>>;
  runLive(batch:Batch):Promise<{response:BatchResponse;subs:Record<string,TokenSubscriptionHandle>}>;
}
export interface TokenSubscriptionHandle { next():Promise<SubscriptionEvent|undefined>; unsubscribe():Promise<void>; }
export interface TokenTx {
  run(op:object):Promise<QueryResult>; rows(op:object):Promise<Array<Record<string,WireValue>>>;
  query(table:string):Query; withRepo(repo:string,table:string):Query; batch(id?:WireValue):Batch;
}
```

`executeWithTouch` is compatibility-only and directly delegates to token execute; it never invokes the SCRAM interner-touch helper. `Batch` remains a pure builder, including bound builders. HTTP exposes only the four allowlisted operations; cursor, subscription and interactive transaction operations fail before send as `not_supported_over_rest`. Admin/unknown operations fail closed as `token_request_denied`. No downgrade. A single transactional HTTP Execute batch remains permitted by TOKEN_API. Token subscription handles are not nominal SCRAM handles.

**Rejected / consequence.** Rejected separate `connectToken`, inferred channel, universal all-capability `ShamirClient`, and incomplete method signatures. Discriminated options preserve SCRAM compatibility and make capabilities explicit.

## 2. Transport seam and protocol correctness

**Observed.** SCRAM WS framing adds a 4-byte prefix (`core/protocol/framing.ts:1-5,108-151`); token WS uses raw msgpack. Existing router demultiplexes rid/push (`core/client/client.ts:436-529,561-610`); `Socket.onClose` loses close metadata (`core/support/platform.ts:14-23`). Buffer early messages until callback registration, with bounded storage.

**Decision.** Isolate token codec/transport; do not route token WS through SCRAM framer. Envelopes are `{sid,rid,req}`, `{rid,res}`, `{rid,error}`; reuse the WS push router after binding. Validate responses before public cast. Keep `query_version:4` where required; do not fake negotiation. Token execution is name-keyed and never invokes SCRAM interner touch/de-interning or sends interner fields/epochs.

```ts
export interface TokenRequestOptions { signal?:AbortSignal; timeoutMs?:number; }
export type TokenHttpRequest = {op:'ping'} | {op:'execute';db:string;batch:BatchRequest;query_version:4} | {op:'wait_for_visibility';db:string;repo:string;version:bigint;visibility_wait:VisibilityWait;query_version:4} | {op:'get_ddl_op_status';db:string;repo:string;table:string;op_id:string};
export type TokenRequest = TokenHttpRequest | {op:'tx_begin';db:string;repo:string;query_version:4;isolation?:IsolationLevel} | {op:'tx_execute';db:string;tx_handle:number;query_version:4;batch:BatchRequest} | {op:'tx_commit';db:string;tx_handle:number;query_version:4;visibility_wait?:VisibilityWait} | {op:'tx_rollback';db:string;tx_handle:number} | CreateCursorRequest | FetchNextRequest | CancelCursorRequest;
export type TokenResponseMap = Record<string,unknown>;
export interface TokenRequestTransport { readonly channel:'http'|'ws'; request(request:TokenRequest,options?:TokenRequestOptions):Promise<TokenResponseMap>; close():Promise<void>; }
export interface TokenHttpRequestInit { method:'GET'|'POST';headers:Readonly<Record<string,string>>;body?:Uint8Array;signal:AbortSignal;redirect:'error';credentials:'omit'; }
export interface TokenHttpResponse {status:number;headers:Readonly<{get(name:string):string|null}>;body:Uint8Array;}
export interface TokenSocket { send(data:Uint8Array):void; onMessage(handler:(data:Uint8Array)=>void):void; onClose(handler:(event:{code:number;reason:string;wasClean:boolean;error?:Error})=>void):void; close(code?:number,reason?:string):Promise<void>; }
export interface TokenPlatform { requestHttp(url:URL,token:ApiToken,init:TokenHttpRequestInit):Promise<TokenHttpResponse>; openTokenSocket(url:URL,token:ApiToken,opts:{tls?:TokenTlsOptions;origin?:string;signal:AbortSignal;connectTimeoutMs:number}):Promise<TokenSocket>; }
export interface TokenPlatformFactoryOptions { tls?:TokenTlsOptions; origin?:string; signal?:AbortSignal; connectTimeoutMs?:number; requestTimeoutMs?:number; }
export type TokenPlatformFactory = (options:NodeTokenHttpConnectOptions|NodeTokenWsConnectOptions|BrowserTokenHttpConnectOptions|BrowserTokenWsConnectOptions)=>TokenPlatform;
```

Заводской адаптер создаёт отдельную платформу для каждого клиента и применяет к реальным запросам/сокетам CA, Origin, AbortSignal и таймеры. `connectTimeoutMs` по умолчанию 10,000 ms ограничивает фазу подключения; HTTP request timeout по умолчанию 35,000 ms. Browser использует native fetch/WebSocket и управляемый браузером Origin, без Node options. Terminal lifecycle отвергает ожидающие запросы, очищает таймеры/очереди и завершает token cursors/subscriptions; reconnect/retry нет.

**Rejected / consequence.** Reusing SCRAM framing or interner helpers violates token wire requirements; isolated codec preserves SCRAM behavior.

## 3. Browser WebSocket alternatives

**Owner requirement.** Browser and Node both support token HTTP and WS. Browser WebSocket cannot set Authorization.

| Alternative | Security and proxy impact | Decision |
|---|---|---|
| Token as second `Sec-WebSocket-Protocol` | May be logged/echoed; replayable; intermediary behavior varies. | Reject. |
| First binary frame on dedicated browser endpoint | No URL/header exposure, but replayable and exposed to page compromise. Requires strict Origin/Host, bounded pre-auth admission, deadline and redaction; proxy must allow WS. | Normative endpoint `/shamir/v1/ws/browser`. |
| Query string | Leaks through logs, copied URLs and diagnostics; replayable credential. | Reject. |
| HTTP-only browser | Does not meet WS requirement. | Not selected. |

Normative sequence, limits and security consequences are specified in TOKEN_API §§4–5, 10, 13 and SECURITY_MODEL §4; this design note does not restate them.

## 4. Node.js specifics

Use built-in `node:http`/`node:https`, client-owned keep-alive Agent, redirect refusal, response byte cap, AbortController/request destruction for connect and full-response deadlines. TLS 1.3, ALPN http/1.1, custom CA, verify by default. Any development bypass must be explicitly named `allowInsecureTlsForDevelopment`; never silently set `rejectUnauthorized:false`. Closing a client aborts its requests and destroys only its own agents. Use existing `ws` package for Node WS custom headers/per-client TLS; built-in Node 22 WebSocket lacks required custom-header/custom-CA controls. Add no runtime dependency. Node >=20 is a candidate, not an established engines/CI result. Browser WS is supported and returns `TokenWsClient`.

**Rejected / consequence.** Ambient/global fetch and built-in WS do not supply required per-client CA/header lifecycle; options must bind to individual transports.

## 5. Browser specifics and packaging

Use fetch + AbortController, `credentials:'omit'`, `redirect:'error'`; no `no-cors`. HTTP is same-origin. Cross-origin Authorization needs CORS/preflight, but TOKEN_API specifies no CORS headers; do not invent support. Recommend same-origin reverse proxy. Browser WS uses `/shamir/v1/ws/browser`, browser-controlled Origin; `ws://` only loopback, prefer WSS. No Node imports in browser entry; preserve Node/browser export split. Keep token in memory by default; persistence is caller choice.

**Observed.** `crates/shamir-client-ts/package.json:7-25` lists scripts/dependencies; `configs/tsconfig.build.json:1-9` gives build config. Native browser WS appears at `src/platform/browser.ts:111-130`; fetch is disabled in `configs/vitest.setup.ts:1-14`, so tests inject it. Existing WS fake: `src/platform/__tests__/ws-subprotocol.test.ts:19-59`. These citations do not establish a bundler graph or browser automation harness. TOKEN_API has no CORS behavior.

**Rejected / consequence.** No cross-origin or browser runtime claims from fakes; deploy same-origin.

## 6. Lifecycle and errors

On terminal WS close reject pending requests, clear timers/queues, fail and clear token subscriptions/cursors. Never reconnect/retry with same token. Close 4010/4011/4012 map `token_sid_mismatch`/`token_revoked`/`token_expired`, terminal. Browser unverified close 4000 maps `authentication_failed`; never alias to sid mismatch or infer native HTTP status. Verified close 4013/4014 maps channel/plaintext denial; 1013 maps `server_busy`. Native has no HTTP status 4000.

`ShamirDbError` retains arbitrary server application codes, separately from the closed transport union. No `| string` extension or `sid_mismatch` alias.

```ts
export class TokenTransportError extends Error {
  readonly code: TokenTransportErrorCode;
  readonly status: number | undefined;
  readonly retryable: boolean;
  readonly retryAfterMs: number | undefined;
  readonly phase: 'connect' | 'request' | 'close';
  constructor(code: TokenTransportErrorCode, message: string,
    options?: { status?: number; retryable?: boolean; retryAfterMs?: number; phase?: 'connect'|'request'|'close' });
}
export type TokenTransportErrorCode =
  | 'token_read_only' | 'token_database_denied' | 'token_request_denied'
  | 'token_channel_denied' | 'token_plaintext_write_denied'
  | 'not_supported_over_rest' | 'invalid_request'
  | 'token_label_exists' | 'token_limit_reached' | 'token_ttl_invalid'
  | 'token_not_found' | 'token_self_service_disabled' | 'token_browser_session'
  | 'permission_denied' | 'hmac_required' | 'hmac_mismatch'
  | 'http_bad_request' | 'unauthorized' | 'http_not_found' | 'http_method_not_allowed'
  | 'network_error' | 'client_timeout' | 'http_timeout' | 'http_body_too_large'
  | 'http_unsupported_media_type' | 'http_bad_headers' | 'rate_limited' | 'server_unavailable'
  | 'protocol_error' | 'client_aborted' | 'tls_configuration_error' | 'connection_closed'
  | 'authentication_failed' | 'token_sid_mismatch' | 'token_revoked' | 'token_expired'
  | 'token_channel_unavailable' | 'first_frame_timeout' | 'preauth_overloaded' | 'server_busy'
  | 'timeout' | 'lock_timeout' | 'tx_conflict' | 'read_only_replica' | 'wal_retention_exceeded';
```

The union includes all TOKEN_API token errors. Arbitrary application codes remain `ShamirDbError`. HTTP mapping: 400 `http_bad_request` (decode application error only for valid msgpack); 401 uniform `unauthorized`; 403 decoded application error; 404/405 `http_not_found`/`http_method_not_allowed`; 408 server `http_timeout`, local deadline `client_timeout` or compatible `ShamirTimeoutError`; 413/415/431 map body-too-large/unsupported-media-type/bad-headers. HTTP 429/503 map `rate_limited`/`server_unavailable`, retain Retry-After advisory. Never automatically replay writes. Network/abort/TLS/wire/other-close map `network_error`/`client_aborted`/`tls_configuration_error`/`protocol_error`/`connection_closed`. Browser-specific close mapping is specified in TOKEN_API §10 and TRANSPORT_WS §6; do not infer local timeout/overload codes from wire.

**Observed.** `core/support/errors.ts:31-80` defines ShamirDbError; `:84-108` distinguishes timeout phase. TOKEN_API §§5, 8, 10, 13 govern validation, status mapping and close lifecycle. Sanitize details; never expose peer body/header/cause.

## 7. Secret hygiene

`ApiToken` stores raw value only in module-private WeakMap, no property/symbol. `from` validates canonical token. `toString`, `toJSON`, `toLocaleString`, and `Symbol.toPrimitive` redact. Node inspect uses a custom hook without Node import in browser. Narrow internal accessor is not package-exported and supplies Authorization or constructs browser auth frame. Never put token/frame in URL, error/cause/stack/log/snapshot/span/metric/audit or echo. Caller owns original string. No deterministic erasure guarantee against same-realm code, heap dumps, extensions or devtools.

**Rejected / consequence.** Public ordinary token fields invite accidental leakage; opaque handle helps but cannot secure compromised same-realm JS.

## 8. Login flow hook — AUTH0

The additive `login(options): Promise<ApiToken>` hook corresponds to TOKEN_LOGIN_HTTP, the normative specification for a proposed feature that is not yet implemented. Login uses a fresh proof and issues an ordinary scoped bearer token; it is distinct from SCRAM session authentication and does not extend shared SCRAM enums or messages. Wire fields, KDF floor, errors and security requirements are defined in TOKEN_LOGIN_HTTP and AUTH_PROTOCOL §5.1.1.

```ts
export interface LoginOptions {
  url: string; username: string; password: string;
  requestTimeoutMs?: number; connectTimeoutMs?: number; signal?: AbortSignal;
  mode?: 'read_only' | 'read_write';
  channels?: 'rest' | 'ws' | 'both';
  databases?: readonly string[] | null;
}
export function login(options: LoginOptions): Promise<ApiToken>;
```

The normative login wire contract and policy are in TOKEN_LOGIN_HTTP; this API reservation is not an implementation claim.

## 9. Test strategy

Proposed test strategy only; no tests are claimed as run. Node HTTP fakes cover auth placement, msgpack, redirects, CA/agent shutdown, abort/deadlines, byte caps, statuses, uniform 401, Retry-After, redaction, name-keyed execution and absence of interner fields. WS fakes cover native header versus browser first-frame transport, early-message buffering, Host/Origin handling, admission/promotion races, malformed frames and terminal cleanup as specified in TOKEN_API §§4–5, 10, 13.

Browser unit fakes cover omitted credentials, refused redirects, no Node imports and same-origin behavior; they do not establish CORS or real browser WS. JS4 proposes Playwright real-browser WSS e2e; harness availability is unverified. No runtime outcome is claimed.

## 10. Stages and folder budgets

**Observed counts.** `src/core/client/` six files plus `__tests__` (7); `src/core/types/` 7; `src/core/protocol/` 5; `src/core/` 7; client tests 6; `src/__tests__/connection/` 6; platform 3. `client.ts` has line numbers through 1000; do not grow it. Counts are a source snapshot, not test evidence.

### 10.1 File layout and folder budgets

Maximum 7 immediate entries per folder, maximum 1000 lines/file; tests under `__tests__`.

- JS1 HTTP: `src/core/types/connection.ts`; `src/core/protocol/token/options.ts`, `errors.ts`, `transport.ts`, `http.ts`, `auth.ts`; `src/platform/token/types.ts`, `http-browser.ts`, `http-node.ts`; `src/entry/api-token.ts`, `token-client.ts`; focused tests.
- JS2 WS: `src/core/protocol/token/ws.ts`; `src/platform/token/socket-node.ts`, `socket-browser.ts`; `src/entry/token-lifecycle.ts`; token-local lifecycle/router tests.
- JS3 public API: `src/entry/connect.ts`, `token-client.ts`, `token-db.ts`; Node `index.ts` and browser `browser.ts` preserve SCRAM overloads and add token overloads; no UA detection.
- JS4 e2e: `src/platform/token/__tests__/`; at most three files in `e2e/`; Playwright Linux/Windows proposal.
- JS5 docs: `crates/shamir-client-ts/docs/README.md` and transport docs; normative amendment records live in the audit note.

Avoid new siblings under full directories; do not add `src/__tests__/token/`. Keep `client.ts` unchanged. Order: JS1, JS2, JS3, JS4, JS5.

Normative browser transport values are maintained in TOKEN_API §10, not copied into this design.

## Proposed spec amendments

Accepted amendments applied to the normative files `TOKEN_API.md`, `TRANSPORT_WS.md`, `SECURITY_MODEL.md`, and `CLIENT_BROWSER.md`. This remains the client design for stages JS1–JS5; normative text is authoritative.
