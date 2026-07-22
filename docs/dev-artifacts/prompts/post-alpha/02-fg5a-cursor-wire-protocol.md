# Brief: FG-5a — cursor wire protocol + spec (#755)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem

2026-07-21 review, P0#5: `QueryResult` materializes all records into a
`Vec`, and both client APIs return arrays. Large result sets are
memory-bounded at both ends. FG-5 (server-side cursors) fixes this; it is
decomposed into 5 sub-tasks. **This brief covers ONLY the first one: the
wire protocol layer.** Do NOT implement the actual cursor engine/session
state (that is FG-5b, a separate future task) and do NOT touch the Rust or
TS SDK's ergonomic streaming wrappers (FG-5c/FG-5d).

## Scope: wire shapes + spec + both builders — NOT the engine

### 1. Wire DTOs (`crates/shamir-query-types/src/`)

- `DbRequest` enum (`wire/db_message.rs:29-209`, `#[serde(tag = "op",
  rename_all = "snake_case")]`): add three new variants —
  - `CreateCursor { query_version: u32, db: String, query: ReadQuery, page_size: u32 }`
    — `query` reuses the existing `ReadQuery` type (same one batch reads
    use), `page_size` bounds how many records come back per page.
  - `FetchNext { cursor_id: CursorId, page_size: u32 }` — `page_size` may
    differ per call (client-controlled backpressure).
  - `CancelCursor { cursor_id: CursorId }` — idempotent: canceling an
    already-closed/unknown cursor is NOT an error (returns
    `CursorClosed` either way).
  - Introduce `pub struct CursorId(pub u64)` (or reuse an existing opaque-id
    newtype pattern already in this module if one exists — check first) as
    an opaque handle; do not leak internal representation.
- `DbResponse` enum (`wire/db_message.rs:212-328`, `#[serde(tag = "kind",
  rename_all = "snake_case")]`): add —
  - `CursorPage { cursor_id: CursorId, page: QueryResult, has_more: bool }`
    — returned by BOTH `CreateCursor` (first page) and `FetchNext`
    (subsequent pages). `page` reuses the existing `QueryResult`
    (`read/query_result.rs:64-109`) — do not duplicate its `records`/`stats`
    shape.
  - `CursorClosed { cursor_id: CursorId }` — returned by `CancelCursor`.
- Error variants — extend `BatchError` (`batch/batch_error.rs`, currently
  ~233 lines) with:
  - `CursorNotFound { cursor_id: CursorId }` — fetch/cancel against an
    unknown id.
  - `CursorExpired { cursor_id: CursorId }` — fetch against an
    idle-timeout-evicted id; MUST be distinguishable from `CursorNotFound`
    on the wire (different error code string) even though FG-5b is what
    actually implements eviction — the wire shape is fixed here.
  - `CursorLimitExceeded { limit: u32 }` — creation past the per-session
    cap (cap enforcement itself lands in FG-5b; this brief only reserves
    the wire error code).
  - Follow the existing enum's derive/display conventions exactly (check
    `thiserror`/`Display` impl already there before adding).

### 2. Server dispatch — compile-safety stub only

`crates/shamir-server/src/db_handler/handler.rs:307` (`impl RequestHandler
for ShamirDbHandler::handle`) matches `DbRequest` **exhaustively** — adding
the three new variants above WILL break compilation until every arm is
handled. Add a match arm for each new variant that returns a placeholder
`DbResponse::Error` with a clear, distinct code (e.g.
`"cursor_not_yet_implemented"`) and a message noting the real
implementation lands in FG-5b. **Do not** wire any actual cursor state,
session storage, or engine calls here — that is out of scope for this
task and belongs to FG-5b. Add a one-line comment at each stub arm citing
"FG-5b" so the next task's implementer finds it immediately.

### 3. Rust query builder (`crates/shamir-query-builder`)

`crates/shamir-query-builder/src/batch/batch.rs` — `Batch::query` (~line
152) and `Batch::query_after` (~line 168) are the existing read-query
builder methods; study how they turn a `ReadQuery` into a `BatchOp`/wire
entry. Add builder support for the new ops — e.g.
`Batch::create_cursor(&mut self, alias, q: impl Into<ReadQuery>, page_size: u32) -> Handle`
mirroring the existing pattern (same alias/handle/dependency machinery).
`FetchNext`/`CancelCursor` are NOT batch entries (they reference an
existing cursor by id, not a fresh query) — decide whether they belong on
`Batch` at all, or are top-level `DbRequest` constructors exposed
separately (e.g. a small free function or a dedicated
`CursorRequest::fetch_next(cursor_id, page_size) -> DbRequest`). Whichever
shape you choose, it must NOT require hand-assembling
`serde_json::Value`/raw wire structs anywhere — repo rule
("Query construction — builder only", see `CLAUDE.md`).

### 4. TS/JS query builder (`crates/shamir-client-ts`)

`crates/shamir-client-ts/src/core/builders/batch.ts` — `Batch.add` (~line
87) is the existing fluent add method; `.build()` at the end constructs
the wire `BatchRequest`. Mirror the Rust builder's new surface here:
`batch.createCursor(alias, query, pageSize)` plus a way to construct
`fetchNext`/`cancelCursor` requests without hand-built JSON. Check
`crates/shamir-client-ts/src/core/` for where standalone (non-batch) wire
requests are already constructed (e.g. how `Ping` or `TxBegin` is built on
the TS side) and follow that existing pattern for `FetchNext`/`CancelCursor`.

### 5. Protocol spec doc

`docs/guide-docs/client-server-protocol-spec/` — add a new `CURSORS.md`
(the directory has no existing per-feature doc naming collision; other
files there are `AUTH_PROTOCOL.md`, `SUBSCRIPTIONS.md`,
`SESSION_RESUMPTION.md`, etc. — match that naming style). Cover:
- cursor lifecycle: create → fetch-next (repeatable) → close/cancel
  (explicit) or idle-timeout (implicit, server-side, detailed in FG-5b —
  reference it here as "see FG-5b" rather than re-describing eviction
  internals).
- wire shapes for all 3 requests / 2 responses / 3 new errors added above,
  with a msgpack-shape example (mirror how `SUBSCRIPTIONS.md` or
  `AUTH_PROTOCOL.md` format their wire examples — check their style
  before writing this one).
- explicitly note in the doc: cursor limit/idle-timeout enforcement is not
  yet live (FG-5b); this document specifies the wire contract in advance
  so FG-5b/c/d/e can build against a stable shape.

## Tests

- Round-trip serde tests in `shamir-query-types` for all 3 new
  `DbRequest` variants and both new `DbResponse` variants (encode → decode
  → equality), following the existing test conventions in that crate for
  `DbRequest`/`DbResponse` (find and mirror an existing wire round-trip
  test file rather than inventing a new pattern).
- New `BatchError` variants: a `Display`/error-code round-trip test
  matching however existing variants are tested.
- Rust builder test(s): building a `CreateCursor`/cursor-related request
  through the new `Batch`/free-function API produces the exact wire shape
  expected (compare against a hand-constructed `DbRequest` value in the
  test, NOT against raw JSON).
- TS builder test(s) (Jest, wherever `shamir-client-ts`'s existing builder
  tests live): same round-trip check for `createCursor`/`fetchNext`/
  `cancelCursor`.
- Server dispatch stub test: sending each of the 3 new ops through
  `ShamirDbHandler::execute`/`handle` returns the placeholder
  `DbResponse::Error` with the documented stub code (proves the
  compile-safety arm is wired end-to-end, not dead code).

## Gate

```
cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-server -- --check
cargo clippy -p shamir-query-types -p shamir-query-builder -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-server --full
```

For the TS side, use whatever the existing `shamir-client-ts` test/lint
scripts are (check `package.json` in that crate/package for the exact
commands — do not guess `npm test` vs `pnpm test` vs a custom script).

All must pass before returning. Stay inside: `shamir-query-types`,
`shamir-query-builder`, `shamir-client-ts`, the one stub arm in
`shamir-server/src/db_handler/handler.rs`, and the new spec doc. Do not
touch the engine (`shamir-engine`), session/tx machinery, or the Rust/TS
SDK streaming wrappers — those are later FG-5 sub-tasks.
