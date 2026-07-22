# Brief: FG-5d — TS SDK streaming cursor (#758)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background test/build invocations.

## Problem

FG-5a (wire protocol), FG-5b (server-side cursor engine), and FG-5c (Rust
SDK `Client::stream_cursor` → `impl futures::Stream`, commit `927a8b33`)
are landed. This task is the TS-side equivalent, in `crates/shamir-client-ts`
— an idiomatic async iterator (`Symbol.asyncIterator`, usable via
`for await (const record of cursor) { ... }`) that issues `fetch_next`
internally as the consumer iterates.

## Code anchors (re-verified against the tree 2026-07-22 — do not re-derive)

- **`crates/shamir-client-ts/src/core/client.ts`**: `ShamirClient.sendDbRequest`
  (private, ~line 504) is the sole request/response primitive — assigns a
  request id, registers a pending-promise slot, encodes/sends via the
  framer, and the background `readLoop` (~line 424) resolves/rejects it by
  id. `readLoop` ALREADY converts any `DbResponse` with `kind === 'error'`
  into a **thrown** `ShamirDbError` (`errors.ts` ~line 60) carrying `code`/
  `detail`/`retryable` — every cursor error code FG-5b defined
  (`cursor_not_found`/`cursor_expired`/`cursor_limit_exceeded`/
  `cursor_temporal_not_supported`) surfaces this way automatically. **Do
  not** invent a `Result`-style wrapped-error item type to mimic the Rust
  SDK — that would be unidiomatic in TS. Let errors propagate as thrown
  exceptions; a `for await` loop that hits one throws naturally, exactly
  like a sync generator.
- **`ShamirClient.execute`** (~line 570) is the existing pattern for "call
  `sendDbRequest`, unwrap the expected `DbResponse` variant, throw on
  mismatch" — mirror it for `create_cursor`/`fetch_next`/`cancel_cursor`.
- **`sendDbRequest` is `private`** — a new class/module outside `client.ts`
  cannot call it directly (TypeScript enforces this at the type level even
  within the same file, unlike a same-module function). **Do not widen its
  visibility.** Instead, add a new **public** method
  `ShamirClient.streamCursor(db, query, pageSize) -> CursorIterator` on the
  class itself (same pattern as `execute`), which constructs the iterator
  and injects a bound closure `(req) => this.sendDbRequest(req)` as a
  constructor parameter — dependency injection, not visibility widening.
- **`crates/shamir-client-ts/src/core/builders/cursor.ts`** (FG-5a):
  `createCursor(db, query, pageSize)`, `fetchNext(cursorId, pageSize)`,
  `cancelCursor(cursorId)` — pure wire-shape constructors, already handle
  `ReadQuery | QueryBuildable` (calls `.build()` if present). **Use these**
  to build the request OBJECTS passed to the injected `sendRequest`
  closure — never hand-assemble `{ op: 'create_cursor', ... }` literals.
- **`crates/shamir-client-ts/src/core/subscription-handle.ts`**
  (`SubscriptionHandle`, lines 26-114) is the ONLY existing async-iterator
  implementation in this SDK — study its shape (`[Symbol.asyncIterator]()`
  returns `this`, `next()` returns `Promise<IteratorResult<...>>`) but note
  it is **push-based** (server-driven events land in a queue). A cursor is
  **pull-based** — `next()` must itself trigger a `fetch_next` round-trip
  when the current page's buffer is drained, not just dequeue.
- **`crates/shamir-client-ts/src/core/db.ts`**: `Db` (the Layer-2
  convenience wrapper) holds `private readonly client: ShamirClient` and
  already has `query()`/`batch()` convenience methods delegating to it
  (~lines 51-84). Add a matching `Db.cursor(query, pageSize) -> CursorIterator`
  delegating to `this.client.streamCursor(this.name, query, pageSize)`.
- **`crates/shamir-client-ts/src/core/types/batch.ts`**: `QueryResult.records`
  is `Array<Record<string, WireValue>>` (~line 200) — there is no distinct
  `QueryRecord` type on the TS side (unlike Rust). The iterator's item type
  is `Record<string, WireValue>` — one record at a time, matching FG-5c's
  per-record granularity (NOT one page at a time) — this is what actually
  fixes the "materializes as array" problem on the TS client side too.
- **No new dependency needed**: TS/Node native async generators/iterators
  are a language feature; `package.json` has no `readable-stream`/
  `it-pushable` and none should be added.
- **Test harness**: `crates/shamir-client-ts/src/__tests__/e2e-harness.ts`
  (`startServer()`, `connectAdmin(HOST, PORT)`, the stale-binary guard) is
  the existing real-server fixture every `e2e-*.test.ts` file uses — mirror
  `e2e-when.test.ts`/`e2e-cond.test.ts`'s structure for a new
  `e2e-cursors.test.ts`. Test runner is **Vitest** (`npm test` → `vitest run`
  per `package.json`), not Jest.

## Design

### 1. `CursorIterator` — new file, dependency-injected transport

`crates/shamir-client-ts/src/core/cursor-iterator.ts`:
```typescript
export class CursorIterator implements AsyncIterableIterator<Record<string, WireValue>> {
  constructor(
    private readonly sendRequest: (req: object) => Promise<Record<string, unknown>>,
    private readonly db: string,
    private readonly query: ReadQuery | QueryBuildable,
    private readonly pageSize: number,
  ) { /* cursorId/buffer/exhausted state starts uninitialized */ }

  [Symbol.asyncIterator](): AsyncIterableIterator<Record<string, WireValue>> { return this; }

  async next(): Promise<IteratorResult<Record<string, WireValue>>> { /* see below */ }
  async return(value?: unknown): Promise<IteratorResult<Record<string, WireValue>>> { /* see below */ }
}
```
- On the FIRST `next()` call, send `createCursor(db, query, pageSize)`
  (built via `builders/cursor.ts`'s constructor, then passed to
  `sendRequest`), unwrap the expected `CursorPageResponse`
  (`kind === 'cursor_page'`), buffer `page.records`, remember `cursor_id`/
  `has_more`.
- On subsequent `next()` calls: pop the next buffered record if any; if the
  buffer is empty and `has_more` was true, send `fetchNext(cursorId,
  pageSize)`, refill the buffer/`has_more` from the new page; if the buffer
  is empty and `has_more` is false, return `{ done: true, value: undefined }`.
- A malformed/unexpected response `kind` should `throw`, matching
  `execute()`'s existing convention — do not swallow it into a `done: true`.

### 2. Cleanup on early break — TS async iterators CAN do this properly (unlike Rust's sync `Drop`)

**This is a meaningful difference from FG-5c's design, not an oversight —
call it out explicitly in the module doc comment.** Rust's `Drop` is
synchronous, so FG-5c could not run a network call on drop and had to rely
on the server-side idle-timeout as the sole backstop for an early-abandoned
stream. TypeScript's `AsyncIterator.return()` is itself `async` and IS
awaited by the JS runtime on every `for await...of` early exit (`break`,
`return`, an exception inside the loop body) — per the language's own
async-iterator protocol. **Implement `return()` to send `cancelCursor`
deterministically** (guarded: no-op if no cursor was ever created, i.e.
`next()` was never called) — this is BETTER than the Rust SDK's story, not
merely equivalent, so do it rather than punting to the idle-timeout
backstop by default. Still document the idle-timeout (60s default, FG-5b)
as the safety net for the rarer case of a caller holding the iterator
without a `for await` loop and simply dropping the reference (no `break`,
no `.return()` called, e.g. never iterated again but never explicitly
closed either) — JS has no deterministic destructors, so THAT specific
case still relies on the server-side backstop, same as Rust.
- Also expose an explicit `async close(): Promise<void>` alias for callers
  driving the iterator manually outside a `for await` loop (calls the same
  logic as `return()`).

### 3. Entry points

- `ShamirClient.streamCursor(db: string, query: ReadQuery | QueryBuildable, pageSize: number): CursorIterator` —
  public method on the class (same file, so it CAN reach the private
  `sendDbRequest`), constructs `new CursorIterator((req) => this.sendDbRequest(req), db, query, pageSize)`.
- `Db.cursor(query: Query | ReadQuery, pageSize: number): CursorIterator` —
  convenience delegating to `this.client.streamCursor(this.name, query, pageSize)`,
  mirroring `Db.query()`/`Db.batch()`'s existing delegation shape.

## Tests (TDD — write failing tests first)

New `crates/shamir-client-ts/src/__tests__/e2e-cursors.test.ts`, using the
`e2e-harness.ts` `startServer`/`connectAdmin` fixture (mirror
`e2e-when.test.ts`'s structure):
- Happy path: seed N rows spanning 3+ pages at a small `pageSize`, iterate
  via `for await (const record of db.cursor(query, pageSize))`, collect
  into an array, assert it matches the seeded set in order (if the query
  has an ORDER BY).
- Early `break` mid-iteration: iterate partway, `break` out of the loop,
  then drive a raw `fetch_next` against the SAME cursor id (via the
  client's `execute`/a direct low-level probe — check whether `sendDbRequest`
  needs a narrow test-only export, or whether the probe can go through
  `client.streamCursor` internals via `cursorId` exposed as a readonly
  property on `CursorIterator` for exactly this purpose, mirroring FG-5c's
  Rust test's `cursor_id()` accessor) and assert it now reports
  `cursor_not_found` — proving `return()` actually reached the server.
- Explicit `close()` outside a loop: same proof, called manually.
- Error propagation: an `AsOf`-temporal query throws `ShamirDbError` with
  `code === 'cursor_temporal_not_supported'` on the first `next()`/first
  loop iteration — assert via `await expect(...).rejects.toThrow(...)` or
  the loop's `try/catch`, not a silently-swallowed `done: true`.
- Empty result set: a query matching zero rows — the `for await` loop body
  never executes, no error.

## Gate

```
npm --prefix crates/shamir-client-ts run typecheck
npm --prefix crates/shamir-client-ts run test
```
(Or `cd crates/shamir-client-ts && npm run typecheck && npm run test` —
check `package.json` for the exact script names before assuming; they were
`typecheck`/`test` as of this brief's writing.) Both must pass. If the
package also has a lint script, run it too and keep it clean.

Stay inside `crates/shamir-client-ts` (new `cursor-iterator.ts` +
`client.ts`'s new `streamCursor` method + `db.ts`'s new `cursor` method +
new test file). Do NOT touch any Rust crate — the wire protocol, server
behavior, and Rust SDK are already complete (FG-5a/b/c) — and do NOT write
the full cross-SDK e2e matrix (FG-5e, a separate later task covering
idle-timeout eviction and per-session cap from BOTH SDKs together).
