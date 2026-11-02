# TOKEN-JS2 WebSocket implementation audit

## Progress

- Existing implementation and audit history retained below; Revision 2 corrections are recorded separately.

- Item 1 DONE at source level: lifecycle handler map/import remain; regression `registers push handlers, routes pushes and clears them on close` exercises handler routing and close cleanup. Test unrun; `entry/__tests__/token-client.test.ts` HTTP harness `requestHttp` repair was made early and must not be reverted.
- Item 2 DONE at source level: lifecycle imports `ShamirDbError` and uses shared HTTP appError for decoded response errors. Regression `rejects wrapped application errors and preserves visibility timeout metadata` covers transport error code, application error code and visibility metadata. Unrun.
- Item 3 DONE at source level: pessimistic isolation rejects before transaction callback or socket send. Regression `rejects pessimistic transactions before invoking the callback or sending` unrun.
- Item 4 DONE at source level: iterator type is `AsyncIterableIterator`; `returns a stable async iterable iterator` checks repeated iterator identity. Unrun.
- Item 5 DONE at source level: subscribe allowlist includes `deliver`, `initial`, `from_version`; `allows subscribe delivery options in token batches` uses subscribe builder and injected request/close transport. HTTP denial remains covered by `rejects nested and wire-only token operations with exact errors before sending`. Unrun. Items 6+ not started.

- DONE: Read committed remediation brief, scoped specs/source, and existing drafts. No prohibited runtime, install, compiler, test, or cargo command was run.
- DONE: Reworked `token-client.ts` shared token operation implementation to accept an injected minimal request transport; WS facade operations now dispatch over `TokenWsLifecycle` rather than HTTP.
- DONE: Fixed lifecycle import extension, sid defensive copies, binding and request timeout/abort behavior, late-socket close, correlation, safe error mapping, early push routing hook, subscription cap, and terminal cleanup.
- DONE: Added WS DB cursor composition via `CursorIterator`, callback-transaction route matching shared `runTransaction`, subscription-handle composition, and top-level transaction request handlers.
- DONE: Expanded response validation for cursor and transaction variants in the shared token transport codec; added in-memory lifecycle/codec Vitest cases.
- DONE: `git diff --check` clean; permitted `python -I scripts/ci/check_layout.py` reports `layout check: OK (0 baselined, 0 hint(s))`.


## Revision 2 correction progress

- Item 1 corrected: lifecycle handler map/import remain; push dispatch and handler cleanup regression is `registers push handlers, routes pushes and clears them on close` in the MemorySocket lifecycle test. Unrun.
- Item 2 DONE at source level: lifecycle imports `ShamirDbError` and uses shared HTTP appError for decoded response errors. Regression `rejects wrapped application errors and preserves visibility timeout metadata` covers transport error code, application error code and visibility metadata. Unrun.
- Item 3 corrected: pessimistic isolation rejection is tested by `rejects pessimistic transactions before invoking the callback or sending`; callback and socket frame count are asserted. Unrun.
- Item 4 corrected: iterator identity is tested by `returns a stable async iterable iterator`. Unrun.
- Item 5 corrected: HTTP denial remains; injected request/close transport test `allows subscribe delivery options in token batches` uses subscribe builder. Unrun.
- Follow-up review corrections: browser abort and timeout paths now fail before socket close; token-send catch removes terminal listeners before close; fake browser socket throws on send for the synchronous-close rejection test. `http-node.test.ts` options declaration corrected to `const options = (port: ...)`. All tests remain unrun; no commands were run.

- DONE: Read the remediation and original transport briefs, audit's prior final report, `CLAUDE.md`, and both TypeScript configs. Current strict config enables `strict` and NodeNext; exact optional/no unchecked/no unused flags are not explicitly set.
- Correction: `src/entry` currently has 7 direct entries (`api-token.ts`, `token-client.ts`, `token-db.ts`, `token-ddl-state.ts`, `token-response.ts`, `__tests__/`, `ws/`); the earlier note's 8-entry assertion was stale after the orchestrator move.
- DONE: Added new `src/entry/ws/token-cursor.ts` subclass, wired it into WS DB, added lifecycle abort signal; implemented subscription as AsyncIterable wrapper with slow-consumer termination and overflow fail-closed; registered active tx handles and adjusted callback result branches. Server terminal close delegates rollback to the server; explicit close best-effort rollbacks tracked transactions before socket close.
- Bounded chunk implementation status: TokenCursor subclass/wiring and terminal-focused tests are present; subscription is now AsyncIterable (not AsyncIterableIterator), reports slow_consumer/queue overflow as terminal error, and caches a stable iterator wrapper; transaction map tracks handles and close attempts rollback while live. Terminal notifications invalidate local transactions and rely on server rollback after disconnect. Node TokenSocket import fixed. These edits are uncompiled and require zero-trust review.
- Bounded chunk status: TokenCursor implemented/wired with terminal abort guard and page/cancel validation; subscription iterator wrapper and slow-consumer/overflow behavior implemented; transaction tracking/cleanup paths edited; Node TokenSocket import fixed. Tests present: `drops buffered and in-flight records after terminal close`, `validates cursor page and cancellation response shapes`, `notifies local resources once on terminal close`, plus HTTP denial regression. None executed.
- Layout checker was run using the sole allowed execution command: `python -I scripts/ci/check_layout.py` -> `layout check: OK (0 baselined, 0 hint(s))`. Manual folders: entry 7 direct entries; entry/ws 4 direct entries; entry/ws/__tests__ 2 files.
- Strict read-through verification identified additional constructor-field typing concern in TokenCursor and client close bookkeeping; partial status stands. No more edits beyond this chunk; CI must typecheck.
- DONE: Added malformed binding variants coverage and corrected the lifecycle test's moved import. Lifecycle response accepts protocol `error: string`, rejects malformed result/error shape and keeps close idempotent.
- PARTIAL: No full strict TS read-through or complete coverage matrix in this continuation. CI typecheck and focused Vitest remain needed.
- The prior final report says eight `src/entry` entries; that count is superseded below by the verified seven-entry count.
- Do not claim this is fully validated or complete: tests/builds were not run by instruction, and there are outstanding correctness risks in strict TypeScript compilation and protocol shape verification. Lifecycle uses timeouts split across socket open and binding rather than a rigorously single elapsed deadline, and subscription routing currently relies on a per-sub router with a global lifecycle count cap; close-time active transaction rollback is not provided. Cursor terminal invalidation and cursor response shape deserve compile/runtime review.
- `TokenWsDb` exposes cursor, tx, and runLive routes; transaction closures reuse the existing commitAttempted semantics. Keep reviewing request type casts and transaction response validation before release.
- `No exports/overloads were added; SCRAM files untouched; git was not mutated.`
- DONE: Follow-up adapter review fixed lifecycle-derived endpoint acceptance, Node upgrade-status draining/mapping, negotiated subprotocol checks, early terminal-close replay, bounded buffered data, idempotent teardown, browser listener-before-auth-send ordering, and WS URL normalization in both platform factories.
- DONE: Expanded focused adapter tests for native auth/TLS config, upgrade status codes, browser binary first frame, close/timeout/abort/queue behavior, and endpoint validation. Test execution remains prohibited.
- Adapter lint-aware pass: restored Node close-event type import; added 16 MiB send/receive limits in both adapters, dynamic handler draining, Node TLS-code classification and retained error listener, terminate/cleanup on Node socket errors, and browser close on protocol/auth-send failure. Browser native adapter now uses explicit structural WebSocket types without DOM global identifiers.
- Platform adapter lifecycle: factories defer browser fetch lookup until HTTP request, reject socket opens after close/lifetime abort, and close sockets they own on factory close/abort. Node/browser HTTP factory WS option normalization retained.
- Final surgical review: stabilized native browser listener wrappers/removal identity; corrected Node mock protocol constructor input to accept `string | string[]`, cast unknown mocks before invoking, and removed self-referential response mock inference. Node timeout/abort/protocol errors now settle before termination; retained socket error listener prevents late unhandled EventEmitter errors. Close test expectation now matches already-closed socket teardown.
- Final limitations: no local strict TS, tests, builds, or npm commands per instruction. Full test execution and strict CI TypeScript validation remain necessary; browser WebSocket has no force-terminate primitive.
- WS dispatch: request-based operation layer is wired to WS lifecycle; HTTP request path is not created for WS clients.
- DB features: cursor, transaction callback, and live subscription methods implemented on WS DB, subject to validation caveats above.
- Tests added: `crates/shamir-client-ts/src/entry/__tests__/token-ws-client.test.ts`, not executed.
- Checks: `git diff --check` and allowed layout script passed. Strict TS build and focused Vitest remain necessary and prohibited in this session.

## Revision 2 items 14–20 progress

- Item 14 DONE source implementation: Node TLS helper return type now includes optional `ALPNProtocols`. No compile run.
- Item 15 DONE source implementation: Node terminal cleanup removes listeners without clearing a saved terminal event; close and socket-error replay regressions added: `replays self-initiated close when registered afterward`, `replays socket error close when registered afterward`. Tests unrun.
- Item 16 DONE source implementation: lifecycle ignores frames with no `rid` while retaining invalid-present-rid protocol validation; regression `ignores unsolicited rid-less errors without ending the session`. Unrun.
- Item 17 DONE source implementation: browser factory preserves TokenTransportError from synchronous factory errors; WS client validates browser plaintext loopback before opening and cleans platform on connection failure. Browser regressions: `preserves local adapter errors and maps server rejection to authentication failure`, `times out while waiting for the binding frame`. Unrun; no runtime verification.
- Item 18 DONE source implementation: lifecycle connection rejection closes platform and preserves original error; regression coverage is pending/not separately added. Unrun.
- Item 19 DONE source implementation: runLive gathers all grants before registering and cleans local subscriptions plus all granted server subscriptions on cap failure; regression `unsubscribes every server grant when the local subscription cap is exceeded`. Unrun.
- Item 20 DONE source implementation: validator removes a batch from the ancestor WeakSet on walk exit; regression `accepts a reused nested batch request but rejects a cyclic one`. Unrun.
- No commands, builds, or tests were run. Items 21+ not started.

- DONE: TokenCursor no longer has uninitialized readonly/unused controller state; create_cursor alone carries query_version, fetch_next/cancel_cursor do not. Cursor result validation now checks records/query-result via `validateTokenBatchResponse` and permits nested wire arrays/objects while requiring name-keyed fields.
- DONE: Confirmed one lifecycle `onTerminal` method remains; `TokenPush.push` now uses the shared `PushKind` union. Subscription `next()` resolves terminal undefined, stable iterator wrapper returns itself, and unsubscribe unregisters lifecycle routing immediately.
- DONE: Moved transaction handle deletion before commit send to avoid rollback after uncertain commit; explicit close remains best-effort rollback only while live.
- DONE: WS origin validation occurs before canonical origin extraction, rejects path/credentials/query/fragment, and handles uppercase WSS without downgrading. Node opening errors preserve sanitized TokenTransportError; browser pre-binding failures remain uniform.
- DONE: Node lifetime abort no longer marks factory closed, preserving later close/agent cleanup. Required `TokenPlatform.openTokenSocket` is being added to source test mocks.
- PARTIAL: Began strict correction pass; fixed TokenCursor page/query-version checks, lifecycle duplicate `onTerminal`, PushKind typing, terminal subscription completion/unregistration, pre-send transaction handle removal, origin-root WS URL validation, Node lifetime-abort state, and socket listener cleanup. These edits remain unverified.
- PARTIAL: Introduced malformed edits in `src/platform/token/__tests__/http-node.test.ts`; current visible text has unresolved `Reply`/`msgpackReply`/`options` and answer helper defects. This file requires repair before any CI review.
- PARTIAL: Missing test coverage not completed (subscription lifecycle, WS factory normalization, redaction/interner/browser graph); no full strict read-through. No executions except prior allowed layout check; no tests/builds/runtime/install/cargo.

- Item 1 — PARTIAL: moved-module imports corrected; response error string support added; missing `validateTokenBatchResponse` import added; HTTP baseline now denies WS-only operations. Remaining lifecycle, transaction, cursor, subscription, terminal cleanup and compile-safety defects are not established as resolved. Evidence: `crates/shamir-client-ts/src/entry/ws/token-lifecycle.ts`, `token-ws-client.ts`, `core/protocol/token/http.ts`.
- Item 2 — PARTIAL: read remediation/original brief, audit, `CLAUDE.md` policy lines 51–106, `tsconfig.json`, `configs/tsconfig.build.json`, plus selected lifecycle/client/codec/platform/HTTP files. A full every-changed-file end-to-end strict read-through was NOT completed. CI typecheck required.
- Item 3 — PARTIAL: expanded binding validation and HTTP-denial tests. Tests added/updated by name: `rejects WebSocket-only cursor and transaction operations before making a request`; existing `validates bindings and canonical browser token framing`. All tests unrun; the required remaining test matrix is incomplete.
- Item 4 — PARTIAL: no new files or moves/deletions. `src/entry` remains over limit at 8 entries; layout/folder counts were not fully audited. No final layout script run.
- Runtime seam: added required `TokenPlatform.runtime` and populated node/browser factories; WS client selects browser pre-auth handling from runtime. This public typing change may expose additional mock implementations requiring update; CI must typecheck all consumers.
- Exports: introduced no new public exports in this continuation; `TokenWsSocketFactory` remains exported and its signature changed to accept `TokenSocketOptions`.
- JS3 connect overloads/package exports remain untouched. No dependencies changed. CI needs strict TypeScript typecheck and all focused Vitest suites, followed by requested full CI matrix. No local builds/tests/runtime/install/cargo were run.
- Zero-trust review requested for changed files: `crates/shamir-client-ts/src/platform/token/types.ts`, `platform/token/http-browser.ts`, `platform/token/http-node.ts`, `platform/token/__tests__/socket-node.test.ts`, `entry/ws/token-lifecycle.ts`, `entry/ws/token-ws-client.ts`, `entry/__tests__/token-ws-client.test.ts`, `core/protocol/token/http.ts`, `core/protocol/token/__tests__/http.test.ts`, `entry/__tests__/token-client.test.ts`, and this audit note.

PARTIAL — JS2 is not complete or ready to use. Added Node/browser socket adapters, codec/lifecycle scaffolding, WS-dispatched shared operations and attempted DB capabilities; focused tests written but unrun. Outstanding: strict-TypeScript defects, cursor/transaction/subscription correctness and complete coverage, plus eight-entry layout violation. Layout command passed but excludes untracked files. JS3 exports/overloads untouched. No builds/tests, git mutation, dependencies, file moves or deletions.

Scope breakdown:
- Implemented at source level: `socket-node.ts` and `socket-browser.ts` adapters/factories/types; codec raw-frame binding, first-80-byte message-pack canonical token; lifecycle request correlation, timeout/abort and terminal skeleton; shared operations dispatch over WebSocket.
- Attempted, not complete: cursor, transaction and subscription DB capabilities. Known unresolved compile/protocol defects include duplicate `Batch` type/value imports, missing `ShamirDbError`, socket-open callback 3-vs-4-argument typing, transaction union mismatches, subscription `next()` returning `IteratorResult` rather than required `Event | undefined`, cursor not being a terminal subclass and missing response validation. Lifecycle tests still use the old connect signature and lose early binding, causing a hang.
- New tests added: `src/entry/ws/__tests__/token-cursor.test.ts` (`drops buffered and in-flight records after terminal close`, `validates cursor page and cancellation response shapes`); `src/entry/ws/__tests__/token-lifecycle-terminal.test.ts` (`notifies local resources once on terminal close`); `src/core/protocol/token/__tests__/http.test.ts` (`rejects WebSocket-only cursor and transaction operations before making a request`). They are unrun.
- Correct current manual direct-entry count: `src/entry` has 7 direct entries; `src/entry/ws` has 4 (including `__tests__`); `src/entry/ws/__tests__` has 2. The older eight-entry statement above describes the earlier state and is superseded by the Revision 1 correction.
- End-to-end lint review was performed repeatedly but did NOT reach compilation-safe state. Typecheck/build/Vitest were forbidden and not run; CI validation is needed after correction.
- Item 6 DONE source implementation: browser negotiation/send failure calls reject before close; regression `fails authentication send before synchronous close settles setup` added, unrun.
- Item 7 DONE source implementation: early terminal event survives listener removal for later registration; browser test `buffers early messages and replays an early terminal close without peer reason` retained. Static review only, unrun.
- Item 8 DONE by static recheck only: existing browser tests `buffers early messages and replays an early terminal close without peer reason`, `rejects a mismatched negotiated protocol before sending credentials`, and `bounds queued messages and resolves repeated close calls together` retained with their oracles. Runtime status unverified.
- Item 9 DONE source implementation: Vitest `ws` mock returns `{ default: MockSocket, WebSocket: MockSocket }`; Node adapter tests retained, unrun.
- Item 10 DONE source implementation: terminal `FakeSocket` buffers early frames and drains them when `onMessage` registers; terminal tests unrun.
- Item 11 DONE source/test recheck: existing `entry/__tests__/token-ws-client.test.ts` request/correlation and response tests remain alongside additions; tests are unrun.
- Item 12 DONE source: HTTP harness now has exactly one `requestHttp` and one `openTokenSocket`; HTTP denial coverage retained. No tests run.

- Correction: item 13's parent comparison confirmed byte-for-byte equality with the historical `http-node.test.ts` baseline. The compile-safe test-only helpers are now confirmed; no tests run.

## Revision 2 items 14–20 progress

- Item 14 DONE source implementation: Node TLS helper return type includes optional `ALPNProtocols`. No compile run.
- Item 15 DONE source implementation: Node terminal listener removal preserves saved close events; regressions `replays self-initiated close when registered afterward` and `replays socket error close when registered afterward` added. Unrun.
- Item 16 DONE source implementation: lifecycle ignores absent-rid frames but still rejects invalid present rids; regression `ignores unsolicited rid-less errors without ending the session`. Unrun.
- Item 17 DONE source implementation: browser adapter preserves local TokenTransportError, client factory enforces browser loopback `ws:` and closes platform on rejection/failure. Regressions `preserves local adapter errors and maps server rejection to authentication failure`, `times out while waiting for the binding frame`. Unrun.
- Item 18 DONE source implementation: failed lifecycle connection attempts close platform while preserving original error. No separate focused regression added; unrun.
- Item 19 DONE source implementation: runLive gathers all grants and on registration failure removes/terminates local handles and unsubscribes all grants. Regression `unsubscribes every server grant when the local subscription cap is exceeded`. Unrun.
- Item 20 DONE source implementation: recursive validation removes completed nodes from ancestor WeakSet; regression `accepts a reused nested batch request but rejects a cyclic one`. Unrun.

## Revision 2 final report

PARTIAL — Revision 2 items 1–21 are implemented at source level, with the requested regression topics authored. Layout and whitespace checks pass; HTTP Node tests match the JS1 baseline exactly. No builds or tests were run. Strict TypeScript/Vitest CI and runtime validation remain outstanding. No dependencies, git state, file moves or deletions were changed.

### Item status (source-level; static review only)

1. DONE — lifecycle push handler registration/cleanup: `crates/shamir-client-ts/src/entry/ws/token-lifecycle.ts:87-92`; `crates/shamir-client-ts/src/entry/__tests__/token-ws-client.test.ts:113-127`.
2. DONE — shared application/visibility error mapping: `crates/shamir-client-ts/src/entry/ws/token-lifecycle.ts:102-105,122-125`; `crates/shamir-client-ts/src/entry/__tests__/token-ws-client.test.ts:129-148`.
3. DONE — pessimistic transaction rejected before callback/send: `crates/shamir-client-ts/src/entry/ws/token-ws-client.ts:103-104`; `crates/shamir-client-ts/src/entry/ws/__tests__/token-ws-client-regression.test.ts:29-36`.
4. DONE — stable async iterable iterator: `crates/shamir-client-ts/src/entry/ws/token-ws-client.ts:37-38`; regression test near end of `token-ws-client-regression.test.ts`.
5. DONE — subscribe delivery options validation/building: `crates/shamir-client-ts/src/entry/token-client.ts:37-38`; `crates/shamir-client-ts/src/entry/ws/__tests__/token-ws-client-regression.test.ts:175-183`.
6. DONE — browser send failure rejects setup before close: `crates/shamir-client-ts/src/platform/token/socket-browser.ts:119-125`; `crates/shamir-client-ts/src/platform/token/__tests__/socket-browser.test.ts:60-67`.
7. DONE — early browser terminal event replay: `crates/shamir-client-ts/src/platform/token/socket-browser.ts:80-83,103-110`; browser adapter regression tests.
8. DONE — browser queue/protocol/close adapter coverage rechecked statically: `crates/shamir-client-ts/src/platform/token/__tests__/socket-browser.test.ts:41-59`.
9. DONE — Node WebSocket mock contract and adapter cases: `crates/shamir-client-ts/src/platform/token/__tests__/socket-node.test.ts:7-20,33-95`.
10. DONE — early lifecycle binding fixture buffering: `crates/shamir-client-ts/src/entry/__tests__/token-ws-client.test.ts:10-34,62-79`.
11. DONE — lifecycle request/correlation/error tests retained and statically rechecked: `crates/shamir-client-ts/src/entry/__tests__/token-ws-client.test.ts:62-175`.
12. DONE — HTTP request harness retained: `crates/shamir-client-ts/src/entry/__tests__/token-client.test.ts` (parent-approved repair; tests unrun).
13. DONE — parent reported historical baseline diff for `http-node.test.ts` at `b07af0c5a` is empty; this was not independently command-verified here.
14. DONE — Node TLS options include ALPN typing: `crates/shamir-client-ts/src/platform/token/socket-node.ts:24-29`.
15. DONE — Node close replay/no-op error listener handling: `crates/shamir-client-ts/src/platform/token/socket-node.ts:51-53,99-100`; corresponding close/error tests in `socket-node.test.ts:53-73`.
16. DONE — absent-rid frame counting and ignoring: `crates/shamir-client-ts/src/entry/ws/token-lifecycle.ts:23,97-100`.
17. DONE — local browser connect error preservation and lifecycle failure mapping: `crates/shamir-client-ts/src/entry/ws/token-lifecycle.ts:40-60`; lifecycle browser tests in `crates/shamir-client-ts/src/platform/token/__tests__/socket-browser.test.ts:68-102`.
18. DONE — original connection error preserved across platform cleanup; browser remote plaintext rejected: `crates/shamir-client-ts/src/entry/ws/token-ws-client.ts:160-181`; regression tests at `token-ws-client-regression.test.ts:61-72`.
19. DONE — runLive tracks all grants/resources and unsubscribes each on cap failure: `crates/shamir-client-ts/src/entry/ws/token-ws-client.ts:112-132`; regression `token-ws-client-regression.test.ts:38-59`.
20. DONE — recursive batch validator permits reused descendants but rejects cycles: `crates/shamir-client-ts/src/entry/token-client.ts:60-76`; regression `token-ws-client-regression.test.ts:74-83`.
21. DONE — close cleanup, tx batch validation, closed subscription reads, async URL rejection, and factory export surface: `crates/shamir-client-ts/src/entry/ws/token-ws-client.ts:26-38,82-88,134-181`; `crates/shamir-client-ts/src/entry/token-client.ts:60-76`; restored close JSDoc in `crates/shamir-client-ts/src/core/client/cursor-iterator.ts:281-288`.

### Added regression coverage and changed-file groups

Added tests: `crates/shamir-client-ts/src/entry/ws/__tests__/token-request-timeout.test.ts` (request timeout and recovery), `token-execute-wire.test.ts` (single name-keyed `executeWithTouch`, query version 4, no interner request), `token-browser-graph.test.ts` (browser source dependency graph), and `token-redaction.test.ts` (token/error redaction). Existing `token-ws-client-regression.test.ts` includes subscription cap cleanup, rejected connection/platform cleanup, remote terminal transaction/subscription/cursor cleanup, push routing, and idempotent unsubscribe. Existing lifecycle/browser regressions cover rid-less responses, subscribe options and binding-timeout/local failure mapping. All tests are UNRUN.

Source edits were made in the token lifecycle/client, Node and browser token socket adapters, shared token client validation, and cursor iterator documentation; test edits/additions are in the corresponding entry/platform suites listed above. The user supplied manual bounds/state: `entry` direct entries 7, `entry/ws` 4, and `entry/ws/__tests__` 7; newly added test files are under 200 lines and production files under 300 lines. Parent reported layout and `git diff --check` pass; no commands were run in this continuation. The parent-reported exact baseline comparison is noted at item 13.

Newly exported helpers: `appError`, `validateBatch`, and `isBrowserLoopback`; lifecycle socket-factory type is internal, with the client socket factory as the sole exported socket factory signature. JS3 exports/overloads are untouched. Historical audit statements above about eight entry files, missing item 18 coverage, or missing imports are superseded by this final status; they are retained as history. Broader response/concurrency assurance and real-browser end-to-end behavior remain for CI/review. No strict compile, runtime, test, install, build, cargo, or npm/node/tsc/vitest validation was performed here.
