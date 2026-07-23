# Server-Side Cursors — Wire Format v1 (FG-5a)

> Status: v1 — normative for the **wire shapes** described here. The engine/session
> state that actually backs a cursor (MVCC as_of snapshot, per-session open-cursor
> cap, idle-timeout eviction) is **NOT implemented yet** — that is FG-5b. This
> document specifies the stable wire contract in advance so FG-5b/c/d/e can build
> against it without another protocol revision. Every request described here is
> currently answered by a placeholder `DbResponse::Error { code:
> "cursor_not_yet_implemented" }` (see §6).

---

## 1. Overview

`QueryResult` (see `OPTIMISTIC_CONCURRENCY.md` / `read/query_result.rs`) materializes
an entire result set into a `Vec` — both ends of the wire hold the full set in memory.
Server-side cursors let a client page through a large result set without either side
ever holding more than one page at a time.

A cursor's lifecycle is three request/response pairs:

1. **Create** — `CreateCursor` opens a cursor over a `ReadQuery` and returns the
   first page.
2. **Fetch-next** (repeatable) — `FetchNext` returns the next page, as many times
   as needed until `has_more == false`.
3. **Close** — either explicit (`CancelCursor`, client-initiated) or implicit
   (server-side idle-timeout eviction — see FG-5b; not yet enforced).

```
client                                              server
  │  CreateCursor{ db, query, page_size }             │
  │ ─────────────────────────────────────────────────►│
  │                                                    │── mint cursor_id ──┐
  │  CursorPage{ cursor_id, page, has_more }           │                    │
  │◄───────────────────────────────────────────────── │◄───────────────────┘
  │                                                    │
  │  FetchNext{ cursor_id, page_size }   (repeatable)  │
  │ ─────────────────────────────────────────────────►│
  │  CursorPage{ cursor_id, page, has_more }           │
  │◄───────────────────────────────────────────────── │
  │           ...                                     │
  │  CancelCursor{ cursor_id }        (explicit close) │
  │ ─────────────────────────────────────────────────►│
  │  CursorClosed{ cursor_id }                         │
  │◄───────────────────────────────────────────────── │
```

Idle-timeout eviction (implicit close with no client-initiated request) is detailed
in FG-5b — referenced here as the counterpart to explicit cancel, not re-described.

---

## 2. `CreateCursor`

Wire discriminator: `"op": "create_cursor"` (`DbRequest`, `#[serde(tag = "op",
rename_all = "snake_case")]`).

```msgpack
{
  "op": "create_cursor",
  "query_version": 2,
  "db": "app",
  "query": { "from": "users", "where": { "op": "eq", "field": ["active"], "value": true } },
  "page_size": 500
}
```

| Field           | Type        | Required | Default                       | Description                                                             |
|-----------------|-------------|----------|--------------------------------|---------------------------------------------------------------------------|
| `query_version` | `u32`       | no       | `CURRENT_QUERY_LANG_VERSION`   | Same query-language version negotiation as `Execute`/`TxBegin`.           |
| `db`            | `string`    | yes      | —                              | Target database name.                                                     |
| `query`         | `ReadQuery` | yes      | —                              | The read query to page through — the SAME shape a batch `Read` op uses.   |
| `page_size`     | `u32`       | yes      | —                              | Records per page for this call, and the default for subsequent `FetchNext` calls that don't override it. |

On success the server replies with the first page as `CursorPage` (§4). Rejections
(query-language version mismatch, unknown db, permission, per-session cursor cap —
FG-5b) surface through the normal `DbResponse::Error` path.

---

## 3. `FetchNext`

Wire discriminator: `"op": "fetch_next"`.

```msgpack
{ "op": "fetch_next", "cursor_id": 7, "page_size": 200 }
```

| Field       | Type   | Required | Description                                                                 |
|-------------|--------|----------|------------------------------------------------------------------------------|
| `cursor_id` | `u64`  | yes      | The cursor minted by `CreateCursor`.                                        |
| `page_size` | `u32`  | yes      | Records for THIS page — may differ from `CreateCursor`'s or any prior `FetchNext`'s `page_size` (client-controlled per-call backpressure). |

On success the server replies with the next page as `CursorPage` (§4). Fetching an
unknown cursor id → `cursor_not_found` (§5); fetching an idle-timeout-evicted cursor
(FG-5b) → `cursor_expired` (§5) — the two are wire-distinguishable so a client can
tell "that id was never valid" apart from "you waited too long".

---

## 4. `CancelCursor`

Wire discriminator: `"op": "cancel_cursor"`.

```msgpack
{ "op": "cancel_cursor", "cursor_id": 7 }
```

| Field       | Type  | Required | Description                          |
|-------------|-------|----------|---------------------------------------|
| `cursor_id` | `u64` | yes      | The cursor to close.                  |

**Idempotent.** Canceling an unknown or already-closed cursor is NOT an error — the
server replies with the same `CursorClosed` envelope either way (mirrors
`UnsubscribeOp`'s idempotency in `SUBSCRIPTIONS.md` §12).

---

## 5. Responses

### 5.1. `CursorPage`

Wire discriminator: `"kind": "cursor_page"` (`DbResponse`, `#[serde(tag = "kind",
rename_all = "snake_case")]`). Returned by BOTH `CreateCursor` (first page) and
`FetchNext` (subsequent pages).

```msgpack
{
  "kind": "cursor_page",
  "cursor_id": 7,
  "page": {
    "records": [ { "id": "u1", "active": true }, { "id": "u2", "active": true } ],
    "stats": { "index_used": null, "records_scanned": 2, "records_returned": 2, "execution_time_us": 140 }
  },
  "has_more": true
}
```

| Field       | Type          | Description                                                                 |
|-------------|---------------|-------------------------------------------------------------------------------|
| `cursor_id` | `u64`         | The cursor this page belongs to.                                             |
| `page`      | `QueryResult` | This page's records/stats — the SAME shape a regular read result uses (`read/query_result.rs`); no duplicated `records`/`stats` schema. |
| `has_more`  | `bool`        | `true` if a further `FetchNext` will return at least one more record; `false` when this was the last page (the server has already released the cursor). |

### 5.2. `CursorClosed`

Wire discriminator: `"kind": "cursor_closed"`. Returned by `CancelCursor`.

```msgpack
{ "kind": "cursor_closed", "cursor_id": 7 }
```

| Field       | Type  | Description                                     |
|-------------|-------|--------------------------------------------------|
| `cursor_id` | `u64` | The cursor id that is now (or already was) closed. |

---

## 6. Errors

Three new `BatchError` variants (surfaced as `DbResponse::Error { code, message }`,
same envelope every other DB-layer failure uses):

| Code                    | Condition                                                                                          |
|-------------------------|------------------------------------------------------------------------------------------------------|
| `cursor_not_found`      | `FetchNext` against a cursor id the server never issued (`CancelCursor` is idempotent instead — see §4). |
| `cursor_expired`        | `FetchNext` against a cursor the server evicted after an idle-timeout (FG-5b). Wire-distinguishable from `cursor_not_found`. |
| `cursor_limit_exceeded` | `CreateCursor` rejected because the session already has the per-session cap (FG-5b) of cursors open. |
| `invalid_page_size`     | CR-A3: `CreateCursor`/`FetchNext` rejected because `page_size` was `0` (would loop the client forever — `has_more` never becomes `false`) or above the server's configured `max_cursor_page_size` cap. **Rejected, never silently clamped** — a client that thinks it got `page_size` rows per page but silently got fewer would misinterpret `has_more` semantics. The cursor itself (if any) is untouched by this rejection — a bad `page_size` on one `FetchNext` call does not close or corrupt an otherwise-valid cursor. |

**Current status (FG-5a):** none of the above three are enforced yet — no engine
state exists to detect them. Every `CreateCursor`/`FetchNext`/`CancelCursor` request
today gets a single placeholder response:

```msgpack
{ "kind": "error", "code": "cursor_not_yet_implemented", "message": "server-side cursors are not implemented yet (FG-5b)" }
```

This placeholder exists purely so the wire shapes above can be exercised end-to-end
(encode → dispatch → decode) before the real engine/session cursor state lands.
Clients integrating against this document should NOT special-case
`cursor_not_yet_implemented` beyond treating it as an ordinary `DbResponse::Error` —
it will disappear once FG-5b ships.

---

## 7. Out of scope for this document

- **Cursor engine/session state** (MVCC as_of snapshot pinning, per-session open-cursor
  cap enforcement, idle-timeout eviction) — FG-5b.
- **Rust SDK ergonomic streaming** (an async `Stream` wrapper over
  create/fetch-next/cancel) — FG-5c.
- **TS SDK ergonomic streaming** (an async iterator wrapper) — FG-5d.
- **End-to-end tests** covering idle-timeout, cancel, and per-session cap behavior —
  FG-5e.

---

## 8. References

Rust types (wire DTOs):

- `crates/shamir-query-types/src/wire/db_message.rs` — `DbRequest::{CreateCursor,
  FetchNext, CancelCursor}`, `DbResponse::{CursorPage, CursorClosed}`.
- `crates/shamir-query-types/src/wire/cursor_id.rs` — `CursorId` (opaque `u64`
  newtype, `#[serde(transparent)]` — round-trips as a bare integer, not a wrapped
  object).
- `crates/shamir-query-types/src/batch/batch_error.rs` — `BatchError::{CursorNotFound,
  CursorExpired, CursorLimitExceeded}`.
- `crates/shamir-query-types/src/read/query_result.rs` — `QueryResult` (reused
  verbatim as `CursorPage.page`).

Server implementation:

- `crates/shamir-server/src/db_handler/handler.rs` — `RequestHandler::handle`'s
  compile-safety stub arms (return `cursor_not_yet_implemented` for all three
  requests today; FG-5b replaces them with real cursor state).

Reference client builders (produce the shapes above — see "Query construction —
builder only" in `CLAUDE.md`):

- Rust: `crates/shamir-query-builder/src/cursor.rs` — `create_cursor`, `fetch_next`,
  `cancel_cursor` free functions (cursor ops are top-level `DbRequest`s, the same
  tier as `TxBegin`/`TxCommit`, not `Batch` entries — see that module's doc comment).
- TypeScript: `crates/shamir-client-ts/src/core/builders/cursor.ts` — `createCursor`,
  `fetchNext`, `cancelCursor`, also exposed as `Batch.createCursor` / `Batch.fetchNext`
  / `Batch.cancelCursor` static helpers.
