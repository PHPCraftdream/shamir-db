# JS1 token HTTP transport audit — 2026-11-01

## Historical progress (superseded)

The progress notes below preserve the earlier chunk record; statements about denied visibility, deferred integration, missing tests/adapters, and validation are superseded by the final audit at the end of this file. The final audit is authoritative.

- Browser adapter completed: `src/platform/token/http-browser.ts` exports the fetch/reader/response structures and `createBrowserTokenPlatform`; bounded streaming, same-origin HTTP(S), authorization-only auth, omitted credentials, redirect refusal, abort/deadline lifecycle, and close are implemented. Tests/builds intentionally not run; adapter/type integration remains for CI.
- Foundation contracts: added a module-private WeakMap ApiToken with canonical base64url tail-bit checks and redacting coercions; transport accessor is a named module export only and is not wired through package entries. Node inspect hook deferred because this code must stay browser-safe without importing Node.
- Scope contracts: introduced HTTP-specific connection option types, token protocol request/error/transport types, and a structural platform HTTP seam; public connect overloads, adapters, clients/DB facade, auth credential exclusion from encoded request, recursive batch allowlisting, and richer response-shape validation remain for later JS1/JS3 work.
- HTTP contract uses existing framing scalar codec helpers, a 16 MiB response cap, static endpoints, no automatic replay, uniform 401, and advisory Retry-After. Deviation: currently broad success shape checks require follow-up against concrete response variants before integration.
- Tests authored: none in this partial foundation pass. CI typecheck/lint and behavioral tests are unrun by instruction; layout gate unrun.
- ApiToken tests added at `crates/shamir-client-ts/src/entry/__tests__/api-token.test.ts`: canonical shape/trailing-bit validation, coercion/inspection/property redaction, safe invalid-input errors, and internal accessor/forged-handle behavior. Tests remain unrun per policy; strict TypeScript compile-read-through completed for the test and source.
- HTTP transport replaced with operation denial, bounded msgpack decoding, typed/sanitized errors, status mapping and terminal close; token options now validate opaque handles, URL origin-root syntax and bounded safe timeouts.
- Added focused `token/__tests__/http.test.ts` protocol and option validation cases. CI must verify strict TS types and framing/error semantics; tests intentionally not run.

The earlier “Final report” section and its duplicated later progress statements were superseded: they incorrectly asserted REST visibility denial and deferred client/adapters. The implementation admits REST visibility requests; see `src/core/protocol/token/http.ts:58` and client handling in `src/entry/token-client.ts:103-107`.

## Final report

Status: **PARTIAL**. The HTTP token transport, platform adapters, token client/DB facade, and seven focused test files exist. Integration/build/test confidence is incomplete because no local builds, tests, or typecheck were run; CI remains required. Package-level `connect()` and client entrypoints remain unchanged, pending JS3.

### Per-item status and evidence

| Item | Status | Evidence / audit finding |
|---|---|---|
| Opaque API token, canonical validation and redaction | DONE | `/crates/shamir-client-ts/src/entry/api-token.ts:1-44`; WeakMap storage, canonical base64url tail bits, coercion redaction, Node inspect symbol, and accessor validation. `ApiToken` does not import Node; inspect is symbol-based. |
| HTTP option and request contracts | DONE | `/crates/shamir-client-ts/src/core/types/connection.ts:1-27`; `/crates/shamir-client-ts/src/core/protocol/token/options.ts:6-65`. Options validate token handle, origin-root HTTP(S) URL, bounded safe timeouts, signal and TLS fields. |
| HTTP request protocol, status mapping and bounded framing | DONE | `/crates/shamir-client-ts/src/core/protocol/token/http.ts:11-20,47-141`; msgpack framing via existing codec, 16 MiB bound, fixed endpoints, authorization header, no replay, uniform 401 and advisory Retry-After capped at 2^31-1 ms. |
| Operation allowlisting and request credential exclusion | DONE | `/crates/shamir-client-ts/src/entry/token-client.ts:26-78,94-100`; recursive batch allowlist and prohibited operation handling; only request payload is encoded and authorization is placed in headers by protocol transport. |
| Client/DB facade and response checks | PARTIAL | `/crates/shamir-client-ts/src/entry/token-client.ts:85-154`; `/crates/shamir-client-ts/src/entry/token-db.ts:10-29`; `/crates/shamir-client-ts/src/entry/token-response.ts:37-78`. Facade exists, and response values receive structural validation, but this is not equivalent to exhaustive validation against every concrete serde response type. |
| Visibility behavior | PARTIAL | `/crates/shamir-client-ts/src/core/protocol/token/http.ts:58,31`; `/crates/shamir-client-ts/src/entry/token-client.ts:103-107`. REST visibility is allowed; `as_of_visibility_timeout` is not specially preserved and is surfaced as `protocol_error`. |
| Error normalization and sanitization | DONE | `/crates/shamir-client-ts/src/core/protocol/token/errors.ts:16-53`; `/crates/shamir-client-ts/src/core/protocol/token/http.ts:12-20,101-118`. Peer messages are replaced with static text. `ShamirDbError` safe unknown application codes require the restricted lowercase identifier regex; unsafe strings are dropped for security. |
| Node HTTP adapter | DONE | `/crates/shamir-client-ts/src/platform/token/http-node.ts:21-139`; bounded response buffering, abort/deadline handling, safe TLS configuration and close. Runtime TLS handshake was not exercised. |
| Browser HTTP adapter | DONE | `/crates/shamir-client-ts/src/platform/token/http-browser.ts:9-117`; fetch/reader contracts, response cap, cancellation, same-origin fixed endpoint and omitted credentials. `connectTimeout` bounds fetch through response headers because browser fetch exposes no TCP-connect phase; it cannot separately bound TCP connection. |
| DDL status types / normalization | DONE | `/crates/shamir-client-ts/src/entry/token-client.ts:109-149`; public DDL operation union additions in `/crates/shamir-client-ts/src/core/types/write/batch.ts` (amended declaration); supported Rust enum values are normalized to public tagged `kind`, including `drop_sorted_index`, `rename_sorted_index`, and `rename_index2`. |
| Tests | DONE (authored; execution unverified) | Seven focused test files: entry tests `api-token.test.ts`, `token-client.test.ts`, `token-response.test.ts`; protocol `http.test.ts`; platform `http-browser.test.ts`, `http-node.test.ts`, `http-node-tls.test.ts`, all under `/crates/shamir-client-ts/src/{entry,core/protocol/token,platform/token}/__tests__/`. Primary named coverage includes token redaction, client operation allowlisting, response validation, HTTP status mapping, browser abort, Node HTTP behavior, and TLS configuration. No test execution is claimed. |
| Public package connection exports | PARTIAL / JS3 | `/crates/shamir-client-ts/src/core/types/connection.ts:3-4` identifies package overloads as deferred. `src/core/client/client.ts`, package entries, and `connect()` were not changed. |
| Name-key wire value compatibility | PARTIAL | `/crates/shamir-client-ts/src/entry/token-response.ts:12-18,37` preserves runtime binary and bigint values, while the legacy `WireValue` type model does not include binary/bigint variants. |

### New module exports

- `src/entry/api-token.ts`: `ApiToken`, `tokenValueForTransport` (module export for internal transport use; not package-entry wired).
- `src/entry/token-client.ts`: `TokenHttpClient`, `TokenPlatformFactory`, `createTokenHttpClient`.
- `src/entry/token-db.ts`: `TokenHttpDb`.
- `src/entry/token-response.ts`: `validateTokenBatchResponse`.
- `src/core/protocol/token/auth.ts`: `authorizationHeader`.
- `src/core/protocol/token/errors.ts`: `TokenTransportErrorCode`, `isTokenTransportErrorCode`, `TokenTransportErrorOptions`, `TokenTransportError`.
- `src/core/protocol/token/http.ts`: `MAX_TOKEN_RESPONSE_BYTES`, `TokenHttpTransport`, and re-export `TokenResponseMap`.
- `src/core/protocol/token/options.ts`: `DEFAULT_TOKEN_REQUEST_TIMEOUT_MS`, `DEFAULT_TOKEN_CONNECT_TIMEOUT_MS`, `NormalizedTokenHttpOptions`, `normalizeTokenHttpOptions`, `TokenRequestOptions`, `TokenHttpRequest`.
- `src/core/protocol/token/transport.ts`: `TokenResponseMap`, `TokenRequestTransport`, `TokenFailure`.
- `src/platform/token/types.ts`: `TokenHttpRequestInit`, `TokenHttpResponse`, `TokenPlatform`, `DEFAULT_TOKEN_RESPONSE_BYTES`.
- `src/platform/token/http-node.ts`: `createNodeTokenPlatform`.
- `src/platform/token/http-browser.ts`: `BrowserTokenHttpReader`, `BrowserTokenHttpResponse`, `BrowserTokenHttpInit`, `TokenFetch`, `createBrowserTokenPlatform`.
- Modified type declarations add `TokenHttpConnectOptions`, `TokenTlsOptions`, `NodeTokenHttpConnectOptions`, `BrowserTokenHttpConnectOptions` in `src/core/types/connection.ts`; DDL union includes the task-relevant Rust variants in `src/core/types/write/batch.ts`.

### Read-through and validation

Read-through covered all authored implementation files listed above, the seven focused test files listed above, and the relevant existing framing codec, visibility types/normalizer, and package entry/client files to verify their status. This records source review only, not compilation or runtime validation.

The observed layout gate returned OK on all three runs; it ignores untracked files. A manual directory/file-budget review found at most 7 direct entries per token module directory, each source file under 1,000 lines, and test child directories with 1/3/3 files (all within the stated limits). No builds, tests, or typecheck were run by the orchestrator. No pass claim is made for them.

### Deviations, remaining work, and ownership

- Public `connect()` and package entry wiring remain for JS3. `src/core/client/client.ts` and package entry files are unchanged; dependencies are unchanged.
- No retry logic, WebSocket transport, token login flow, E2E coverage, or JS3/JS2/deferred docs work was implemented.
- Client response checks remain structural rather than exhaustive serde-equivalent validation; optional further structural validation/test coverage is not claimed exhaustive.
- `as_of_visibility_timeout` has no specialized variant and becomes `protocol_error`; name-key runtime values can include binary/bigint although the existing type model is legacy-limited.
- Node TLS handshake behavior has not been exercised. Browser connect timeout cannot isolate TCP connection establishment.
- The untracked root `PROGRESS.md` was accidentally created by the worker during this run. It was left unchanged after discovery because deletion was forbidden; owner must delete it before commit.
- No source, test, `PROGRESS.md`, git state, commit, or push was modified by this audit operation, apart from the accidental `PROGRESS.md` creation noted above.

|
