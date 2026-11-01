# TOKEN-JS0 — token client design for `shamir-client-ts`

**Status: design proposal only.** No implementation, builds, or tests are claimed. This note contains the ten design decisions and exact proposed normative amendments; amendments are not applied here.

## Owner decisions needed

- **Pre-auth limits and close codes:** approve proposed 256 global / 8 per subnet pre-auth permits, 5-second first-frame deadline, 256-byte cap, browser close `4000`, verified denials `4013`/`4014`, and post-upgrade overload `1013`.
- **Host and proxy authority:** approve explicit `host_authority_allowlist`; trust forwarded authority/address only from configured trusted proxies.
- **Node baseline:** Node 20+ is a candidate; confirm against supported CI/runtime policy.
- **CORS:** no cross-origin support proposed. Recommend same-origin proxy; any CORS change needs a separate decision.
- **AUTH0:** preserve additive `login(options): Promise<ApiToken>`. `TOKEN_LOGIN_HTTP.md` is absent from this worktree; future authoritative specification must define wire fields and KDF parameters.

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
| First binary frame on dedicated browser endpoint | No URL/header exposure, but replayable and exposed to page compromise. Requires strict Origin/Host, bounded pre-auth admission, deadline and redaction; proxy must allow WS. | Recommend `/shamir/v1/ws/browser`. |
| Query string | Утечка через журналы, скопированные URL и диагностику; попадание в историю или `Referer` зависит от последующей навигации/пересылки и не происходит автоматически при создании WebSocket; токен допускает повторное предъявление. | Отклонить. |
| HTTP-only browser | Does not meet WS requirement. | Interim fallback only. |

Native `/shamir/v1/ws` сохраняет аутентификацию по заголовку Authorization и проверку токена/лимита активных соединений до `101`: отказ credentials остаётся единым HTTP 401; проверенный отказ — HTTP 403; исчерпание лимита — HTTP 503. Browser проверяет Host/Origin и резервирует pre-auth ресурсы до `101`, затем проверяет первый кадр. Только непроверенные ошибки browser закрываются общим кодом 4000; проверенные отказы канала/незашифрованной политики — 4013/4014; исчерпание активного лимита после upgrade — 1013. Нативные статусы не меняются.

Origin браузера обязателен, единственен, не равен `null` и имеет допустимый синтаксис; точное нормализованное значение сверяется с `browser_origin_allowlist` без wildcard (ошибка/отсутствие — 400, несовпадение — 403). Host должен единственным значением совпадать с нормализованным `host_authority_allowlist` (отсутствие/дубликат/запрет — 400). Forwarded authority/address доверяется только настроенным trusted proxies; `X-Forwarded-Host` нельзя принимать вслепую; `X-Forwarded-For` используется для подсетевой атрибуции только от доверенных прокси. Cookie-аутентификации нет.

Предлагаемые (новые) лимиты browser pre-auth admission: 256 глобально, 8 на подсеть, атомарное резервирование до upgrade; исчерпание — HTTP 503. После проверки атомарно acquire уже существующие конфигурируемые active caps `tokens.max_sockets_per_token` (по умолчанию 8) и `tokens.max_sockets_per_owner` (по умолчанию 32), publish в TokenSessionStore, перепроверить revoke/expiry при promotion, затем освободить pre-auth permit. Освободить все permits на каждом terminal path. Native проверяет/acquires те же конфигурируемые active caps до upgrade. Исчерпание browser active cap после upgrade — 1013, native — HTTP 503. Эти значения не являются существующими defaults.

Первый browser-кадр — один бинарный msgpack map с единственным ключом `token`, канонический ASCII-токен длиной 71; без повторов/неизвестных ключей и завершающих байтов. Обычный размер 80 байт (map 1 + key 6 + str8 prefix 2 + token 71). До выделения памяти предел 256 байт; срок ожидания 5 секунд. Compression выключен, masking обязателен; text, fragmentation, RSV, oversize и дополнительные application frames до проверки отвергаются. Не более 8 control frames за 5 секунд, каждый ≤125 байт; они не продлевают ожидание. Не создавать неограниченные очереди и не вызывать DB handler до проверки. Любые непроверенные malformed/unknown/invalid/expired/revoked/owner-invalid случаи закрываются общим `4000 authentication_failed`, с одинаковыми публичной причиной/timing class и dummy work по TOKEN_API. Отдельный subnet failure limiter не отклоняет verified token; после upgrade HTTP 429 нет. Проверенный отказ `channels` закрывается 4013, plaintext-policy denial — 4014. Admission resource failure может быть различим от credential failure.

Предлагаемый учёт browser pre-auth payload: 64 KiB на сокет = 16 MiB глобально и 512 KiB на подсеть. Отдельный резерв учёта admission — 256 KiB на соединение = 64 MiB при 256; он не гарантирует полного потребления памяти процесса/ядра. Оба токен-маршрута используют `shamir-v1`; нативный клиент не отправляет кадр токена. Browser `ws://` разрешён только на loopback; нативные серверные plaintext rails и SCRAM WS endpoints остаются без изменений.

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

The union includes all TOKEN_API token errors. Arbitrary application codes remain `ShamirDbError`. HTTP mapping: 400 `http_bad_request` (decode application error only for valid msgpack); 401 uniform `unauthorized`; 403 decoded application error; 404/405 `http_not_found`/`http_method_not_allowed`; 408 server `http_timeout`, local deadline `client_timeout` or compatible `ShamirTimeoutError`; 413/415/431 map body-too-large/unsupported-media-type/bad-headers. HTTP 429/503 map `rate_limited`/`server_unavailable`, retain Retry-After advisory. Never automatically replay writes. Network/abort/TLS/wire/other-close map `network_error`/`client_aborted`/`tls_configuration_error`/`protocol_error`/`connection_closed`. `first_frame_timeout` and `preauth_overloaded` are local/reserved, not inferred wire codes. Browser 4000 is generic auth failure; 4013/4014 and 1013 map as above. Existing retry classification remains advisory.

**Observed.** `core/support/errors.ts:31-80` defines ShamirDbError; `:84-108` distinguishes timeout phase. TOKEN_API §§5, 8, 10, 13 govern validation, status mapping and close lifecycle. Sanitize details; never expose peer body/header/cause.

## 7. Secret hygiene

`ApiToken` stores raw value only in module-private WeakMap, no property/symbol. `from` validates canonical token. `toString`, `toJSON`, `toLocaleString`, and `Symbol.toPrimitive` redact. Node inspect uses a custom hook without Node import in browser. Narrow internal accessor is not package-exported and supplies Authorization or constructs browser auth frame. Never put token/frame in URL, error/cause/stack/log/snapshot/span/metric/audit or echo. Caller owns original string. No deterministic erasure guarantee against same-realm code, heap dumps, extensions or devtools.

**Rejected / consequence.** Public ordinary token fields invite accidental leakage; opaque handle helps but cannot secure compromised same-realm JS.

## 8. Login flow hook — AUTH0

`TOKEN_LOGIN_HTTP.md` does not exist in the worktree. Do not invent wire fields, budgets/defaults or section numbers. Future authoritative specification titles are provisionally *Endpoints*, *Handshake state*, *Channel binding*, *Issued token*, *Errors*. It will define username/fresh nonce/metadata; challenge salt/KDF/server nonce/opaque handle; finish handle/client proof/requested scope; and result. Password never crosses network; verify server proof if specified. Apply the protocol-defined client KDF floor symbolically; reject unsupported, below-floor or over-cost parameters before expensive derivation without copying server budgets/defaults. Credential failures are generic; other errors typed only per future specification.

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

This additive API reservation does not assert a current endpoint or implementation.

## 9. Test strategy

All tests below are proposals, none run. Node HTTP fake server: auth placement, msgpack, redirects, CA/agent shutdown, abort/deadlines, byte caps, all statuses, uniform 401, Retry-After, redaction, name-keyed execution, no interner fields. WS fakes: native header vs browser first-frame, early greeting buffering, Host/Origin rejection, admission and promotion races, valid 80-byte frame, malformed/duplicate/unknown/trailing/text/oversize/fragment/RSV/extra frames all generic 4000, no pre-auth DB, permit release, 4010–4014, 1013, binding/envelopes, terminal cursor/subscription cleanup and redaction.

Browser unit fakes cover omitted credentials, refused redirects, no Node imports and same-origin behavior; they do not establish CORS or real browser WS. JS4 proposes Playwright real-browser WSS e2e, Linux/Windows, same-origin HTTPS proxy, token operations/cancellation/revocation/cursor/transaction/subscription and no URL/log exposure. Existing citations do not prove automation harness availability. No runtime outcome is claimed.

## 10. Stages and folder budgets

**Observed counts.** `src/core/client/` six files plus `__tests__` (7); `src/core/types/` 7; `src/core/protocol/` 5; `src/core/` 7; client tests 6; `src/__tests__/connection/` 6; platform 3. `client.ts` has line numbers through 1000; do not grow it. Counts are a source snapshot, not test evidence.

### 10.1 File layout and folder budgets

Maximum 7 immediate entries per folder, maximum 1000 lines/file; tests under `__tests__/`.

- JS1 HTTP: `src/core/types/connection.ts`; `src/core/protocol/token/options.ts`, `errors.ts`, `transport.ts`, `http.ts`, `auth.ts`; `src/platform/token/types.ts`, `http-browser.ts`, `http-node.ts`; `src/entry/api-token.ts`, `token-client.ts`; focused tests.
- JS2 WS: `src/core/protocol/token/ws.ts`; `src/platform/token/socket-node.ts`, `socket-browser.ts`; `src/entry/token-lifecycle.ts`; token-local lifecycle/router tests.
- JS3 public API: `src/entry/connect.ts`, `token-client.ts`, `token-db.ts`; Node `index.ts` and browser `browser.ts` preserve SCRAM overloads and add token overloads; no UA detection.
- JS4 e2e: `src/platform/token/__tests__/`; at most three files in `e2e/`; Playwright Linux/Windows proposal.
- JS5 docs: `crates/shamir-client-ts/docs/README.md` and transport docs; amendments await approval.

Avoid new siblings under full directories; do not add `src/__tests__/token/`. Keep `client.ts` unchanged. Order: JS1, JS2, JS3, JS4, JS5.

### 10.2 Capacity accounting

| Budget | Value | Scope |
|---|---:|---|
| Pre-auth admission | global 256; subnet 8 | Browser before 101; proposed |
| Active sockets | configurable `tokens.max_sockets_per_token` default 8; configurable `tokens.max_sockets_per_owner` default 32 | Existing shared caps, both endpoints |
| Browser first frame | 256 bytes; 5 seconds | Before verification; proposed |
| Payload accounting | 64 KiB/socket; 16 MiB global; 512 KiB/subnet | Browser pre-auth; proposed |
| Admission accounting | 256 KiB/connection; 64 MiB at 256 | Accounting only; not total-memory guarantee |

| Этап | Нативный `/shamir/v1/ws` | Браузерный `/shamir/v1/ws/browser` |
|---|---|---|
| До upgrade | Проверка Authorization и active cap; исчерпание → HTTP 503. | Проверка Host/Origin и pre-auth admission; исчерпание → HTTP 503. |
| После 101 | Сразу binding; token frame не нужен. | Проверить первый кадр и общий активный лимит. При исчерпании: close `1013`, завершение без кадра привязки. При успехе: переход в активное состояние и кадр привязки. |

Pre-auth/resource значения — предложения, не утверждение о существующих defaults. Admission accounting не гарантирует всю память процесса/ядра; overhead проверяется при реализации.

## Proposed spec amendments (do not apply)

Каждая поправка указывает точный anchor, target, action и полный русский текст. Эти изменения предлагаются, но нормативные документы здесь не редактируются.

### Amendment 1 — TOKEN_API.md §1 Каналы

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §1.

**Exact anchor:**
```text
| 2 | WebSocket | `kind: http`, `tls: false` | `GET /shamir/v1/ws` |
```

**Action:** replace row; following WSS row «как 2» inherits both endpoint choices.

**New text:**
```text
| 2 | WebSocket | `kind: http`, `tls: false` | `GET /shamir/v1/ws` (нативный), `GET /shamir/v1/ws/browser` (браузерный; только loopback без TLS) |
```

### Amendment 2 — TOKEN_API.md §1 граница listener-а

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §1.

**Exact anchor:**
```text
Граница безопасности — **тип listener-а**, не путь. Listener `kind: http` обслуживает только токены и никогда не создаёт SCRAM-сессий; listener `kind: tcp|ws|unix` никогда не принимает токен. Путь `/shamir/v1/ws` (токен) отличается от `/shamir/v1` и `/shamir/v1/browser` (SCRAM, TRANSPORT_WS) и обслуживается только `kind: http`. Listener `kind: http` не отдаёт никаких маршрутов без токена: `/shamir/v1/health`, `/shamir/v1/version`, `/admin/*`, метрики на нём не существуют (404).
```

**Action:** replace the paragraph only.

**New text:**
> Граница безопасности — **тип listener-а**, не путь. Listener `kind: http` обслуживает только токены и никогда не создаёт SCRAM-сессий; listener `kind: tcp|ws|unix` никогда не принимает токен. Эндпоинты `/shamir/v1/ws` (нативный bearer-WS) и `/shamir/v1/ws/browser` (браузерный bearer-WS) доступны только на `kind: http`; SCRAM-эндпоинты `/shamir/v1` и `/shamir/v1/browser` остаются отдельными и не принимают токены. На `kind: http` нет маршрутов без токена: `/shamir/v1/health`, `/shamir/v1/version`, `/admin/*` и метрики отвечают 404. Исключение browser WS ограничено upgrade и первым кадром bearer-аутентификации; публичные маршруты не добавляются, к БД до проверки токена обращения нет.

### Amendment 3 — TOKEN_API.md §4 Представление токена

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §4.

**Exact anchor:**
```text
## 4. Представление токена
```

**Action:** replace only the three numbered items; keep heading and following section.

**New text:**
```text
1. REST и нативный `GET /shamir/v1/ws` принимают ровно один заголовок `Authorization`, схему `Bearer` (регистр схемы не важен), один пробел и токен. Отсутствующий или повторный заголовок, иная схема, лишние пробелы, длина токена не 71 символ или неканоничный base64url обрабатываются по §5; отсутствие заголовка отдельно сохраняет действующее правило лимитера.
2. Только браузерный `GET /shamir/v1/ws/browser` получает токен из единственного первого бинарного WS-кадра после `101`. Кадр — одна msgpack-карта с единственным строковым ключом `token` и строковым значением каноничного токена §2: 71 ASCII-символ. Неизвестные/повторные ключи и завершающие байты запрещены; размер обычного кодирования — 80 байт, предел до выделения памяти — 256 байт. На этом маршруте заголовок `Authorization` не является источником токена.
3. На обоих маршрутах токен не принимается из URL, query string, cookie, HTTP-тела или `Sec-WebSocket-Protocol`. Единственное исключение из header-only аутентификации — первый браузерный WS-кадр из пункта 2; ошибки до и после upgrade определены в §5 и §10.
```

### Amendment 4 — TOKEN_API.md §5 Проверка

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §5.

**Exact anchor:**
```text
Выполняется **по заголовкам запроса**, до чтения тела и до `101`:
```

**Action:** replace opening sentence only; retain existing steps 1–8.

**New text:**
```text
Для REST и нативного `/shamir/v1/ws` шаги 1–8 выполняются по заголовкам до чтения тела и до `101`; действующие HTTP-статусы и правила лимитера сохраняются.

Браузерный `/shamir/v1/ws/browser` сначала проверяет Host/Origin и резервирует pre-auth ресурсы (§10), затем выполняет upgrade. Токен первого бинарного кадра проходит разбор и проверки шагов 2–5, включая фиктивный HMAC для неизвестного id, constant-time MAC, состояние владельца и строгое `created_at_ns > tickets_invalid_before_ns(owner)`. Политика шага 6 проверяется после подтверждения секрета: отсутствие `ws`/`both` в `channels` даёт close `4013 token_channel_denied`; plaintext policy — `4014 token_plaintext_write_denied`.

Любая ошибка непроверенного кадра или credentials (malformed, неизвестный id, неверный секрет, отзыв, срок, недействительный владелец) даёт только close `4000 authentication_failed` с одинаковыми публичной причиной и временным классом. Сохраняются subnet failure limiter и аудит aggregation/sampling; HTTP `429` после upgrade не выдаётся, прошедший проверку токен лимитер не отклоняет. До успешной проверки DB handler не вызывается. Сессия из шага 7 создаётся/публикуется только после резервирования общего active cap по последовательности §10.
```

### Amendment 5 — TOKEN_API.md §10 WebSocket

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §10.

**Exact anchor:**
```text
## 10. WebSocket (каналы 2 и 4)
```

**Action:** replace the section body after the anchored heading up to, but not including, `## 11. Транспортная безопасность`; preserve that following heading.

**New text:**
```text
**Конечные точки и подпротокол.** Оба токен-маршрута используют подпротокол `shamir-v1`: нативный `/shamir/v1/ws`, браузерный `/shamir/v1/ws/browser`. Неверный upgrade или подпротокол даёт HTTP `400` до `101`.

**Нативное обновление соединения.** Нативный маршрут принимает `Authorization: Bearer <token>`; проверка по TOKEN_API §5 и атомарный общий лимит активных соединений выполняются до `101`. Настройки `tokens.max_sockets_per_token` (по умолчанию 8) и `tokens.max_sockets_per_owner` (по умолчанию 32) конфигурируемы, а не фиксированы клиентом. Невалидный токен сохраняет единый HTTP `401` и отдельный лимитер неудач; проверенные отказы политики канала/незашифрованного соединения дают `403`; исчерпание active cap — HTTP `503`. Нативный `Origin` необязателен; если задан, сверяется с `browser_origin_allowlist`; пустой список запрещает запрос с Origin (`403`). После успешного `101` сразу отправляется существующий кадр привязки. Нативный клиент не посылает кадр токена.

**Проверка Host/Origin и резервирование до browser upgrade.** Браузерный маршрут до `101` требует единственный корректный Origin и допустимый Host. Отсутствующий, `null`, повторный или некорректный Origin даёт HTTP `400`; несовпадение с точным allowlist Origin без wildcard — `403`; пустой список запрещает все запросы. Host нормализуется как authority и сверяется с явным `host_authority_allowlist`; отсутствующий, повторный или неразрешённый Host даёт `400`. Пересылаемый authority принимается только от настроенных доверенных прокси; `X-Forwarded-Host` без такого доверия не принимается. `X-Forwarded-For` для подсетевого лимитера используется только от доверенных прокси, иначе учитывается адрес пира. Cookie-аутентификации нет.

До browser upgrade атомарно резервируются предлагаемые разрешения учёта pre-auth: 256 глобально, 8 на подсеть; исчерпание даёт HTTP `503`. Эти числа не утверждают наличие существующих настроек. До проверки credentials разрешения не связываются с токеном/владельцем.

**Первый кадр браузера.** После `101` ожидается ровно один первый бинарный кадр не позднее 5 секунд; предел 256 байт проверяется до выделения буфера. Это одна msgpack-карта `{ "token": <71-символьный ASCII-токен> }`, без неизвестных или повторных ключей и завершающих байтов. Обычный размер — 80 байт: карта 1 + ключ 6 + префикс str8 2 + токен 71. Сжатие отключено, маскирование клиента обязательно; текстовые, фрагментированные, RSV, чрезмерно большие кадры и дополнительные application-кадры до проверки запрещены. Не более 8 управляющих кадров размером до 125 байт за период ожидания; они не продлевают срок ожидания. Не создавать неограниченных очередей.

**Непроверенные ошибки browser и перевод в активное состояние.** Выполняются разбор и проверки §5: фиктивный HMAC для неизвестного id, сравнение MAC за постоянное время, проверка состояния токена и порога владельца. Любая непроверенная ошибка кадра или учётных данных (неизвестный, неверный, истёкший, отозванный токен или недействительный владелец) закрывается общим `4000 authentication_failed` с одинаковыми публичной причиной и временным классом; `1003`/`1009` не раскрывают категорию ошибки до проверки. Сохраняются агрегация/сэмплирование аудита и отдельный лимитер подсетевых неудач; HTTP `429` после upgrade не выдаётся, проверенный токен лимитер не отклоняет. До проверки обработчик БД недостижим.

После проверки секрета отсутствие `ws`/`both` в `channels` закрывается `4013 token_channel_denied`; нарушение политики незашифрованного соединения — `4014 token_plaintext_write_denied`. Затем атомарно резервируется общий активный лимит. Его исчерпание после upgrade закрывается `1013 server_busy`. Сессия публикуется в TokenSessionStore с повторной проверкой отзыва/срока при переходе в активное состояние; отзыв в гонке даёт `4011`, истечение — `4012`. Разрешение учёта pre-auth освобождается при успешном переходе или любом завершении; все активные разрешения освобождаются на каждом терминальном пути. Отказ по ресурсам может отличаться от отказа учётных данных.

| Этап | Нативный `/shamir/v1/ws` | Браузерный `/shamir/v1/ws/browser` |
|---|---|---|
| До upgrade | Проверка Authorization и общего активного лимита до `101`; исчерпание → HTTP `503`. | Проверка Host/Origin и резервирование pre-auth ресурсов до `101`; исчерпание → HTTP `503`. |
| После `101` | Сразу кадр привязки; клиентский кадр токена не нужен. | Проверить первый кадр и общий активный лимит. При исчерпании: close `1013`, завершение без кадра привязки. При успехе: переход в активное состояние и кадр привязки. |

**Привязка и установленная сессия.** Кадр привязки `{ "ok": true, "sid": bin(32), "expires_at_ns": N }` отправляется нативному клиенту сразу после `101`, браузерному — только после проверки кадра и перехода в активное состояние; до этого браузер не получает кадр привязки или конверт приложения. Каждый последующий бинарный WS-кадр содержит одну msgpack-карту: запрос `{sid:bin(32),rid?:u32,req:bin(msgpack DbRequest)}`, ответ `{rid?:u32,res:bin(msgpack DbResponse)}`, ошибка `{rid?:u32,error:string}`; push использует `PushEnvelope` из SUBSCRIPTIONS.md. `sid` случаен, ненулевой, 32 байта, принадлежит одному соединению и хранится в общем TokenSessionStore; чужой sid закрывает `4010 token_sid_mismatch`. Лимит запросов после аутентификации возвращает конверт `rate_limited`, соединение остаётся открытым.

Перед каждым обработчиком состояние токена перепроверяется (Acquire). Отмена снимает подписки, прекращает push, откатывает транзакции и закрывает курсоры. Срок сессии — минимум из `token.expires_at_ns` и времени открытия плюс `tokens.session_max_age_hours`; часы переводятся в наносекунды до сложения. SCRAM 24 часа и `SESSION_IDLE_TTL` не применяются; отдельного таймера простоя нет. Heartbeat и предел кадра данных 16 MiB соответствуют TRANSPORT_WS; после аутентификации текстовый кадр закрывается кодом `1003`, превышение размера — `1009`; остановка — `1000`. Отзыв сначала публикуется, затем отменяются сессии; опубликованный отзыв блокирует новые запросы, запрос, уже прошедший проверку, может завершиться. Настройки `tokens.max_sockets_per_token` (по умолчанию 8) и `tokens.max_sockets_per_owner` (по умолчанию 32) остаются конфигурируемыми, а не жёстко заданными; токен-сокеты не расходуют лимит SCRAM-сессий владельца.

### 10.3 Учёт сокетов

Общий активный лимит обоих bearer-маршрутов задаётся `tokens.max_sockets_per_token` (по умолчанию 8) и `tokens.max_sockets_per_owner` (по умолчанию 32). Нативный маршрут проверяет и резервирует лимит до `101`; исчерпание возвращает HTTP `503`. Браузерный маршрут проверяет и резервирует его после успешной проверки первого кадра и до публикации активной сессии; исчерпание после upgrade закрывает `1013 server_busy`. Значения остаются конфигурируемыми. Токен-сокеты не расходуют лимит SCRAM-сессий владельца.

**Остальные коды закрытия.** Коды TRANSPORT_WS §6 продолжают действовать с указанными здесь исключениями. `4000` используется для непроверенной браузерной аутентификации и не применяется на нативном token-пути; `4001 unsupported_version` не используется ни одним token endpoint. Коды `4010`–`4012` сохраняют существующие значения; `4013`/`4014` и `1013` определены выше. Остановка сервера закрывает соединения кодом `1000`.

**Предлагаемый учёт ресурсов.** Учёт browser pre-auth payload: 64 KiB на сокет, 16 MiB глобально и 512 KiB на подсеть. Отдельный admission accounting: 256 KiB на соединение, 64 MiB при 256; это не гарантия полной памяти процесса/ядра, накладные расходы измеряются при реализации. Browser `ws://` разрешён только для loopback; нативные серверные plaintext rails остаются без изменений. Этот браузерный порядок не меняет существующие нативные HTTP-статусы `401`/`403`/`429`/`503`.
```

### Amendment 6 — TOKEN_API.md §13 Аудит, метрики, логи

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §13.

**Exact anchor:**
```text
## 13. Аудит, метрики, логи
```

**Action:** append.

**New text:**
> Браузерный WS: первый кадр токена, его байты и представления, подобные Authorization, не записываются в журналы, spans, метрики или аудит — ни при успехе, ни при ошибке. Сохраняются существующие агрегация и сэмплирование событий неудачной аутентификации; секрет и MAC не добавляются.

### Amendment 7 — TOKEN_API.md §14 Вне scope v1

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §14.

**Exact anchor:**
```text
mTLS, OAuth/OIDC, JWT, cookies, CORS, JSON-тела, resource-style REST и OpenAPI, браузерные WebSocket-клиенты (браузер не может задать `Authorization` для WebSocket), административные операции через токен, транзакции / курсоры / подписки по REST, репликация токенов между узлами, квоты на токен сверх rate limit, статистика использования токена, ротация `token_key`, токен как параметр подключения в `shamir-client` и `shamir-client-ts` (T9).
```

**Action:** replace paragraph only.

**New text:**
> mTLS, OAuth/OIDC, JWT, cookies, CORS, JSON-тела, resource-style REST и OpenAPI, административные операции через токен, транзакции / курсоры / подписки по REST, репликация токенов между узлами, квоты сверх rate limit, статистика использования токена, ротация `token_key`, токен как параметр подключения в `shamir-client` (T9). Браузерный bearer WebSocket на выделенном `/shamir/v1/ws/browser` включён; браузерный SCRAM-путь не изменяется.

### Amendment 8 — TOKEN_API.md §15 Test Checklist

**Target:** `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, §15.

**Exact anchor:**
```text
## 15. Test Checklist
```

**Action:** append.

**New text:**
> Браузерный WS проверить на реальных сокетах в Linux и Windows: Host/Origin и отказы до `101`; исчерпание нативного active cap → HTTP `503` до upgrade; browser pre-auth cap → HTTP `503`; browser active cap → `1013`; timeout, валидный кадр 80 байт, неверные/повторные/неизвестные ключи, завершающие байты и кадр больше 256 байт → общий `4000` без обращения к БД; проверенный отказ канала/незашифрованной политики → `4013`/`4014`; отзыв/истечение → `4011`/`4012`; освобождение разрешений и отсутствие кадра в журналах, spans и аудите.

### Amendment 9 — TRANSPORT_WS.md scope

**Target:** `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md`, opening paragraph.

**Exact anchor:**
```text
Только `wss://`. Plain WS удалён в v1 (browser требует TLS, native может использовать TCP plain).
```

**Action:** replace opening paragraph; retain SCRAM scope clarification following it.

**New text:**
> Только SCRAM WebSocket требует `wss://`; plain SCRAM WS удалён в v1. Bearer WS на listener `kind: http` — отдельный TOKEN_API: нативный `/shamir/v1/ws` использует заголовок Authorization, браузерный `/shamir/v1/ws/browser` — первый бинарный кадр после upgrade. Для browser bearer `ws://` разрешён только loopback; native bearer plaintext подчиняется неизменённым правилам TOKEN_API. SCRAM endpoints не изменяются.

### Amendment 10 — TRANSPORT_WS.md §1 Профили

**Target:** `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md`, §1.

**Exact anchor:**
```text
## 1. Профили
```

**Action:** append.

**New text:**
> Bearer WebSocket на listener `kind: http` — отдельный протокол TOKEN_API, не профиль SCRAM: нативный `/shamir/v1/ws` использует Authorization до upgrade, браузерный `/shamir/v1/ws/browser` — первый бинарный кадр после upgrade. User-Agent не выбирает маршрут. SCRAM `/shamir/v1` и `/shamir/v1/browser`, их binding и handshake остаются без изменений.

### Amendment 11 — TRANSPORT_WS.md §6 Close Codes

**Target:** `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md`, §6.

**Exact anchor:**
```text
Коды 4010-4012 выдаёт только listener `kind: http`; `4003` остаётся `heartbeat_timeout` на обоих видах соединений.
```

**Action:** append after sentence.

**New text:**
> Только для bearer `/shamir/v1/ws/browser`: `4000 authentication_failed` — любая непроверенная ошибка кадра/credentials; `4013 token_channel_denied` и `4014 token_plaintext_write_denied` — только после проверки; `1013 server_busy` — исчерпание active cap после upgrade. Семантика кодов закрытия SCRAM не меняется; native bearer сохраняет HTTP-отказы до upgrade.

### Amendment 12 — TRANSPORT_WS.md §8 Frame Size Limits

**Target:** `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md`, §8.

**Exact anchor:**
```text
Идентичны TCP. Pre-auth `≤ 4 KB`, data `≤ 16 MB`. Превышение → close 1009.
```

**Action:** replace sentence.

**New text:**
> Для SCRAM WS пределы как TCP: pre-auth `≤ 4 KB`, data `≤ 16 MB`, превышение → `1009`. Для bearer browser endpoint TOKEN_API §10 отдельно задаёт первый кадр до проверки: максимум 256 байт, ожидание 5 секунд; некорректный/чрезмерный кадр или истечение времени → `4000`, не `1009`. После аутентификации предел bearer data frame — `≤ 16 MB`.

### Amendment 13 — TRANSPORT_WS.md §9 Origin

**Target:** `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md`, §9.

**Exact anchor:**
```text
## 9. Origin Header
```

**Action:** append.

**New text:**
> Эти правила относятся к SCRAM. Браузерный bearer endpoint `/shamir/v1/ws/browser` дополнительно проверяет Origin и authority Host до upgrade по TOKEN_API §10; политика SCRAM не меняется. Bearer endpoint не использует cookie-аутентификацию или выбор режима по User-Agent.

### Amendment 14 — SECURITY_MODEL.md §4 Non-Guarantees

**Target:** `docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md`, §4.

**Exact anchor:**
```text
## 4. Non-Guarantees
```

**Action:** append.

**New text:**
> **Браузерный bearer WebSocket.** На `/shamir/v1/ws/browser` bearer принимается первым бинарным кадром после upgrade, не из URL, cookie или subprotocol. Это исключает утечку через URL/referrer, но кадр остаётся повторно предъявляемым bearer credential и уязвим к компрометации страницы, расширений и TLS-потока. До проверки ограничены число соединений, размер кадра и время ожидания; проверяются Origin/Host; обращения к БД невозможны. Меры ограничивают злоупотребление ресурсами и CSWSH, но не обеспечивают proof-of-possession, защиту от компрометации страницы или дополнительную защиту от повторного предъявления. Browser `ws://` только loopback; предпочтителен WSS.

### Amendment 15 — SECURITY_MODEL.md §5 RFC 6750

**Target:** `docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md`, §5.

**Exact anchor:**
```text
| RFC 6750 (Bearer Token Usage) | Subset | Только заголовок `Authorization`; токен не принимается из query и тела; `WWW-Authenticate: Bearer` на 401 |
```

**Action:** replace row.

**New text:**
```text
| RFC 6750 (Bearer Token Usage) | Subset | REST и нативный WS используют `Authorization`; браузерный WS использует единственный первый бинарный кадр на выделенном endpoint; токен не принимается из query, cookie или тела запроса; `WWW-Authenticate: Bearer` на HTTP 401 |
```

### Amendment 16 — CLIENT_BROWSER.md §6 Connection Code

**Target:** `docs/guide-docs/client-server-protocol-spec/CLIENT_BROWSER.md`, §6.

**Exact anchor:**
```text
## 6. Connection Code (Pseudo-API)
```

**Action:** append.

**New text:**
> Браузерный bearer-клиент использует token API отдельно от SCRAM `ShamirClient`: строит абсолютный origin-root URL без userinfo/query/fragment/path, использует HTTP того же origin, `credentials: 'omit'` и `redirect: 'error'`. Для WS подключается к `/shamir/v1/ws/browser` и отправляет токен одним первым бинарным msgpack-кадром после upgrade; токен не помещается в URL, cookie, Origin или subprotocol. Browser API не предоставляет TLS-настроек Node или пользовательского Origin. CORS не включается: cross-origin HTTP остаётся недоступным без отдельного изменения серверной политики.

### Amendment 17 — CLIENT_BROWSER.md §9 Test Checklist

**Target:** `docs/guide-docs/client-server-protocol-spec/CLIENT_BROWSER.md`, §9.

**Exact anchor:**
```text
## 9. Test Checklist (release blockers)
```

**Action:** append.

**New text:**
> Браузерный bearer WS следует отдельно проверить в реальном браузере через same-origin WSS: токен отсутствует в URL/журналах, корректный первый бинарный кадр устанавливает сессию, несовпадение Origin/Host отклоняется, истечение ожидания и повреждённые кадры закрываются общим `4000`, проверенный отказ канала — `4013`, исчерпание active cap — `1013`. Псевдо-API не считать существующим SDK-контрактом до реализации.
