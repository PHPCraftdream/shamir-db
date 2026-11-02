# TOKEN-JS3 audit report

## Current API decision

The SCRAM `ConnectOptions` interface remains intact. Token `connect()` returns the `PublicTokenClient` HTTP/WS union where channel cannot be narrowed, with per-channel overloads for literal schemes/channels. This follows the JS0/revision-1 decision; universal nominal `ShamirClient` branding is not an outstanding requirement.

## Revision 1 item status (source-level)

| Item | Status and source evidence |
|---|---|
| 1 | DONE — `crates/shamir-client-ts/src/entry/token-db.ts:20-24` declares `TokenLiveSubscription`'s async iterator protocol; regression `returns a stable async iterable iterator` is in `src/entry/ws/__tests__/token-ws-client-regression.test.ts`. |
| 2 | DONE — SCRAM `ConnectOptions` remains an interface and `ScramConnectOptions` aliases it (`src/core/types/connection.ts:78-103`); token platform option unions are declared separately (`:14-21`). |
| 3 | DONE — browser entry has SCRAM and token overloads, including the general options overload (`src/browser.ts:13-18`). |
| 4 | DONE — public `TokenWsClient` and `PublicTokenClient` contracts are in `src/entry/ws/token-client.ts:14-20`; implementation is declared against the public contract in `src/entry/ws/token-ws-client.ts:57`. |
| 5 | DONE — Node and browser entries each declare per-channel HTTP/WS overloads before generic token overloads (`src/index.ts:13-17`, `src/browser.ts:13-17`). |
| 6 | DONE — token client types are exported from the Node entry (`src/index.ts:50-55`) and browser entry (`src/browser.ts:31-37`). |
| 7 | DONE — shared types barrel exports only cross-platform connection types at `src/core/types/index.ts:40`; platform-specific contracts are exported by their respective entries. |
| 8 | DONE — Node validates scheme/channel and TLS compatibility without a blanket plaintext-remote rejection (`src/platform/connect/node-connect.ts:27-34`); browser plaintext WS checks use loopback validation (`src/platform/connect/browser-connect.ts:23-29`). |
| 9 | DONE — browser connector takes `BrowserTokenConnectOptions` (`src/platform/connect/browser-connect.ts:11`). |
| 10 | DONE — invalid-option tables and error codes, SCRAM forwarding, public contract assertions, and Node/browser graph isolation checks are authored in `src/platform/connect/__tests__/public-connect.test.ts:79-125` and `src/entry/ws/__tests__/token-browser-graph.test.ts:18-70`. |
| 11 | DONE — entries import connector functions directly (`src/index.ts:4`, `src/browser.ts:4`); `platform/node.ts` and `platform/browser.ts` expose platform factories, not connector re-exports. Connector factory spies are used by the public connect tests (for example `public-connect.test.ts:80-81`). |

## Revision 2 item status (source-level)

| Item | Status and source evidence |
|---|---|
| 1 | DONE — `sourceGraph(file, visited, forbidNode)` conditionally scans Node imports and propagates the flag recursively; browser graphs remain strict, and the Node entry graph passes `false` (`src/entry/ws/__tests__/token-browser-graph.test.ts:18-70`). Browser graph sources also reject `ws` package imports/exports and assert connector isolation. |
| 2 | DONE — public connect coverage is restored and strengthened in `src/platform/connect/__tests__/public-connect.test.ts`: Node HTTP scheme execution and explicit WS; browser HTTP/WS execution; REST denials; push/unsubscribe; and scheme/channel matrix. Details follow. |
| 3 | DONE — explicit `[options, code]` Node/browser invalid-option cases, including required auth/token, TLS, timeout, and channel mismatch cases, are at `public-connect.test.ts:79-116`. |
| 4 | DONE — the non-invoked `probe` assigns all eight Node/browser per-channel and generic overload results (`public-connect.test.ts:238-259`). Public entry type imports for `TokenCursor` and browser `SubscriptionEvent` are also present there. |
| 5 | DONE — `TokenCursor` is exported by both entries (`src/index.ts:53`, `src/browser.ts:34`); browser also exports `SubscriptionEvent` (`src/browser.ts:36`), defined at `src/core/client/subscription-router.ts:14-19`. |
| 6 | DONE — `browser-connect.ts` imports and uses `isBrowserLoopback` at its plaintext WS guard (`src/platform/connect/browser-connect.ts:7,29`); its duplicate helper is removed. |
| 7 | DONE — this report was rewritten to reflect current source-level status, restored/dropped coverage, and validation limits. |

## Restored and retained test coverage

`src/platform/connect/__tests__/public-connect.test.ts` contains the following current tests:

- `selects Node HTTP from scheme and runs ping/query requests through REST`: asserts ping and query results, normalized opaque `NodeApiToken`, HTTP factory channel and normalized `https://db.example/` URL, exact `/shamir/v1/ping` and `/shamir/v1/request` paths, and no socket frames.
- `uses explicit Node WS and never sends HTTP requests`: asserts ping/query results, WS factory options and opaque token, no HTTP requests, and two socket frames.
- `connects in browser over HTTP and WS using normalized string tokens`: asserts factory options with `BrowserApiToken`, normalized URLs, HTTP laziness before ping, ping/query results, request paths, and WS-only frames.
- `denies REST-only operations before sending through both public entries`: subscription batch denial through `execute`, cursor/transaction/runLive denials for both entries, no HTTP requests, and transaction callbacks not invoked.
- `delivers pushes through both public entries and unsubscribes`: direct subscription `.next()` event assertions, exact decoded `batch.queries._unsub` payload `{ unsubscribe: 7 }` for each entry, and no HTTP requests.
- `routes two subscriptions and makes repeated unsubscribe idempotent` remains as distinct multi-subscription/idempotence coverage.
- `derives or honors matching channel choices across Node and browser schemes` checks derived and explicit selection for eight runtime/scheme rows, with a fresh socket/platform per client.
- Invalid Node coverage includes 17 explicit options/code rows: malformed path/query/userinfo, TLS array, aborted signal, username/password with token, forged token handle, missing/unknown auth, TLS on plaintext HTTP/WS, insecure TLS false, fractional timeout, HTTPS with WS channel, and WSS/WS with HTTP channel. Browser has 11 invalid rows including browser-only option rejection and insecure remote plaintext WS.
- `accepts Node remote plaintext HTTP and WebSocket URLs` is a positive acceptance check for remote plaintext; the historical `http://127.attacker.example` rejection was dropped. Historical duplicate push/subscription copies were not retained; one push test spans both entries and the two-subscription test is distinct.
- The overload test's `probe` is declared and referenced with `void probe`, never invoked; it assigns Node/browser scheme-derived HTTP, explicit HTTP, WS, and `String(...)` generic URL results.

## Validation and layout

The orchestrator ran the permitted layout gate: `python -I scripts/ci/check_layout.py` reported `layout check: OK (0 baselined, 0 hint(s))`. Manual entry counts: `platform/connect` has 3 direct entries; its `__tests__` folder has 1; `entry/ws/__tests__` has 7. No new files were added; edited source/test files remain below 1000 lines. No builds, tests, typechecks, or compiler runs were performed. CI strict TypeScript NodeNext declaration checking and Vitest 1.6 remain pending. All DONE statuses above mean source-level completion, not runtime/compiler verification.

## Revision 2 progress

- Item 1 DONE — configurable Node-import scan and strict browser `node:`/`ws` graph assertions; connector isolation retained.
- Item 2 DONE — restored Node/browser transport and public API coverage, with restored/dropped details above.
- Item 3 DONE — explicit error-code tables and requested validation options added.
- Item 4 DONE — non-invoked overload result probe covers both entries.
- Item 5 DONE — `TokenCursor` exported from both entries; browser `SubscriptionEvent` exported from its verified definition.
- Item 6 DONE — shared browser loopback helper reused.
- Item 7 DONE — this accurate audit replaces obsolete status claims.

## Final report

Implemented all 7 revision-2 items at source level across `index.ts`, `browser.ts`, `browser-connect.ts`, `public-connect.test.ts`, `token-browser-graph.test.ts`, and the audit `DESIGN.md`.
Restored public-connect coverage, added overload probes and exports, corrected graph traversal, and shared browser loopback validation. Layout check passed; no local builds, tests, or typechecks ran.
Remaining: CI strict TypeScript/declaration checking and Vitest execution. No git mutation, file moves, or deletions.
