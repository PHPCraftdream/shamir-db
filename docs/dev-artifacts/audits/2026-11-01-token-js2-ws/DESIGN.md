# TOKEN-JS2 WebSocket implementation audit

## Progress
- DONE: Read committed remediation brief, scoped specs/source, and existing drafts. No prohibited runtime, install, compiler, test, or cargo command was run.
- DONE: Reworked `token-client.ts` shared token operation implementation to accept an injected minimal request transport; WS facade operations now dispatch over `TokenWsLifecycle` rather than HTTP.
- DONE: Fixed lifecycle import extension, sid defensive copies, binding and request timeout/abort behavior, late-socket close, correlation, safe error mapping, early push routing hook, subscription cap, and terminal cleanup.
- DONE: Added WS DB cursor composition via `CursorIterator`, callback-transaction route matching shared `runTransaction`, subscription-handle composition, and top-level transaction request handlers.
- DONE: Expanded response validation for cursor and transaction variants in the shared token transport codec; added in-memory lifecycle/codec Vitest cases.
- DONE: `git diff --check` clean; permitted `python -I scripts/ci/check_layout.py` reports `layout check: OK (0 baselined, 0 hint(s))`.

## Deviations / limitations
- The earlier layout violation remains: the worktree contains 8 `src/entry` sibling files from the prior draft. No entry siblings were added in this continuation; files were not moved/deleted.
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

## Final report

PARTIAL — JS2 is not complete or ready to use. Added Node/browser socket adapters, codec/lifecycle scaffolding, WS-dispatched shared operations and attempted DB capabilities; focused tests written but unrun. Outstanding: strict-TypeScript defects, cursor/transaction/subscription correctness and complete coverage, plus eight-entry layout violation. Layout command passed but excludes untracked files. JS3 exports/overloads untouched. No builds/tests, git mutation, dependencies, file moves or deletions.

Scope breakdown:
- Implemented at source level: `socket-node.ts` and `socket-browser.ts` adapters/factories/types; codec raw-frame binding, first-80-byte message-pack canonical token; lifecycle request correlation, timeout/abort and terminal skeleton; shared operations dispatch over WebSocket.
- Attempted, not complete: cursor, transaction and subscription DB capabilities. Known unresolved compile/protocol defects include duplicate `Batch` type/value imports, missing `ShamirDbError`, socket-open callback 3-vs-4-argument typing, transaction union mismatches, subscription `next()` returning `IteratorResult` rather than required `Event | undefined`, cursor not being a terminal subclass and missing response validation. Lifecycle tests still use the old connect signature and lose early binding, causing a hang.
- Coverage is incomplete. New tests: `src/entry/__tests__/token-ws-client.test.ts`, `src/platform/token/__tests__/socket-node.test.ts`, and `src/platform/token/__tests__/socket-browser.test.ts`; unrun.
- Eight direct `src/entry` entries violate the seven-entry limit; resolving this requires deletion/movement, which was prohibited. Layout script passed but ignores untracked files and therefore does not vindicate the manual entry count.
- End-to-end lint review was performed repeatedly but did NOT reach compilation-safe state. Typecheck/build/Vitest were forbidden and not run; CI validation is needed after correction.
- JS3 overloads/package exports untouched; no dependencies changed, commits/pushes, file deletion or moves.
