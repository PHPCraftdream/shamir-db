# Server-Side Cursors — Wire Format v1 (FG-5a)

> Status: v1, backed by REAL engine/session state (FG-5b shipped). A cursor is
> pinned to an MVCC snapshot at `CreateCursor` time and pages through results via
> keyset-style (or row-count-offset, when there's no simple single-column `ORDER
> BY`) bookmarking; `FetchNext` re-runs the pinned-snapshot read with an advancing
> bookmark rather than holding a live engine stream open across round-trips. Only
> `Temporal::Latest` queries may open a cursor — `AsOf`/`History` are rejected
> outright with `cursor_temporal_not_supported` (§6), not silently downgraded. A
> per-session open-cursor cap and an idle-timeout reaper are both enforced (§1,
> §2). Wave A hardening (ACL checks on open/fetch, no leaked registration of an
> already-exhausted first page, `page_size` validation, byte-budget + per-page
> size-cap coverage, and a tie-safe keyset bookmark for duplicate `ORDER BY`
> values) has also landed — see §6 and the R-6 cost-model note below for what has
> NOT yet landed. CR-D2 (#783): a keyset-eligible `ORDER BY` column containing a
> `Null`/missing value is now detected at `CreateCursor` time and the WHOLE
> cursor falls back to row-count-offset pagination instead — see §1.1. A
> mixed-type or `NaN`-containing `ORDER BY` column is NOT detected and remains a
> known limitation (silent row loss) — see §1.1 and `KNOWN_LIMITATIONS.md` §6.

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
   (server-side idle-timeout eviction — enforced today, see below).

```
client                                              server
  │  CreateCursor{ db, query, page_size }             │
  │ ─────────────────────────────────────────────────►│
  │                                                    │── mint cursor_id ──┐
  │  CursorPage{ cursor_id, page, has_more }           │                    │
  │◄───────────────────────────────────────────────── │◄───────────────────┘
  │                                                    │
  │  FetchNext{ cursor_id, page_size? }  (repeatable)  │
  │ ─────────────────────────────────────────────────►│
  │  CursorPage{ cursor_id, page, has_more }           │
  │◄───────────────────────────────────────────────── │
  │           ...                                     │
  │  CancelCursor{ cursor_id }        (explicit close) │
  │ ─────────────────────────────────────────────────►│
  │  CursorClosed{ cursor_id }                         │
  │◄───────────────────────────────────────────────── │
```

Idle-timeout eviction (implicit close with no client-initiated request) is enforced
by a background reaper task (`crates/shamir-server/src/cursor_registry.rs`,
`spawn_reaper_task`): a cursor idle longer than `security.cursors.idle_timeout_secs`
(default 60s) is evicted on the next sweep (default interval 5s), releasing its
pinned MVCC snapshot. A `FetchNext` racing the reaper against a just-evicted cursor
gets the wire-distinguishable `cursor_expired` (§6), not `cursor_not_found`.

**Cost model — cursors reduce wire/client memory, not server-side per-page execution
cost.** Each `FetchNext` re-executes a full pinned-version table scan server-side to
reach the next page: the underlying `AsOf` read path enumerates the table's current
id set, individually looks up each matched id at the pinned version, and — when the
query has an `ORDER BY` (the normal cursor case) — sorts the entire matched set
before slicing off one page. The cost model is therefore **O(table) per page, not
O(page_size)**, and server-side peak memory for a single page's execution is
approximately the size of the FULL matching set, not just that page. This matters
for a consumer deciding between a cursor and a single large `Read` with
`max_result_size_bytes` headroom: a cursor bounds wire/client-side memory across
many round-trips, but it does not make any individual `FetchNext` cheaper or
lighter on the server than the equivalent one-shot read over the same data.

### 1.1. Keyset pagination mode: data-dependent fallback and residual limitations

A cursor over a query with a single, simple (top-level-field) `ORDER BY` column
pages via a **keyset boundary-filter bookmark** (`field >= last_seen_value`, ASC —
`<=`, DESC) for lower per-page memory pressure than the row-count-offset
alternative. Whether that scheme is SAFE for a given column is a property of the
column's **data**, not the query's shape — this section documents what the server
checks (and does not check) before committing to keyset mode.

- **`Null` / missing `ORDER BY` value (CLOSED, CR-D2 #783):** a `Null` or
  entirely-absent value in the `ORDER BY` column cannot be proven `>=`/`<=`
  anything, so a naive keyset boundary filter silently excludes that row from
  every page after the first — the cursor would report a clean `has_more: false`
  having silently dropped it. The server closes this unconditionally: at
  `CreateCursor` time, once the query is otherwise keyset-eligible, it runs ONE
  additional cheap `WHERE <order_by_field> IS NULL LIMIT 1` existence check
  against the SAME pinned snapshot the first page reads. If that probe finds ANY
  row, the WHOLE cursor is pinned to row-count-offset pagination from creation —
  before the first page is even fetched — so a null-containing `ORDER BY` column
  can never reach the keyset boundary-filter bug. This is purely a server-side,
  data-dependent decision: **a client cannot request or predict which mode a
  given cursor will use**, and the two modes have different cost profiles (the
  offset fallback re-scans from a row-count position each `FetchNext` rather than
  seeking via the boundary filter) — a consumer building strict cost expectations
  around "my `ORDER BY` column is simple, so this must be keyset-mode" should not
  assume that holds once the column's data can contain nulls.
- **Mixed-`QueryValue`-type or `NaN`-containing `ORDER BY` column (STILL OPEN, not
  fixed by CR-D2):** the SAME class of bug applies to a column holding more than
  one `QueryValue` variant (e.g. some rows `Int`, some `Str`) or an `F64` column
  containing `NaN` — the boundary comparison is unresolvable for those rows too,
  and they are silently dropped once the scan passes page 1. Unlike the `Null`
  case, there is no existing cheap filter primitive (e.g. an "is this field a
  different type than X" or "is this field NaN" check) to probe for this at
  `CreateCursor` time, so it is NOT detected and NOT fixed — see
  `KNOWN_LIMITATIONS.md` §6 for the precise, current-state disclosure. Avoid
  keyset-eligible cursors over an `ORDER BY` column that may hold mixed types or
  `NaN` until this is closed.

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
enforced via `security.cursors.max_cursors_per_session`, default 16) surface through
the normal `DbResponse::Error` path.

---

## 3. `FetchNext`

Wire discriminator: `"op": "fetch_next"`.

```msgpack
{ "op": "fetch_next", "cursor_id": 7, "page_size": 200 }
```

`page_size` is OPTIONAL (CR-B3, #769) — a request that omits it entirely still
decodes cleanly:

```msgpack
{ "op": "fetch_next", "cursor_id": 7 }
```

| Field       | Type             | Required | Description                                                                 |
|-------------|------------------|----------|------------------------------------------------------------------------------|
| `cursor_id` | `u64`            | yes      | The cursor minted by `CreateCursor`.                                        |
| `page_size` | `u32` (optional) | no       | Records for THIS page — when present, may differ from `CreateCursor`'s or any prior `FetchNext`'s `page_size` (client-controlled per-call backpressure). When ABSENT, the server falls back to the `page_size` given at `CreateCursor` time (that call's own field doubles as every subsequent omitted-`FetchNext`'s default — see §2). |

On success the server replies with the next page as `CursorPage` (§4). Fetching an
unknown cursor id → `cursor_not_found` (§5); fetching an idle-timeout-evicted cursor
→ `cursor_expired` (§5) — the two are wire-distinguishable so a client can
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

`BatchError` variants (surfaced as `DbResponse::Error { code, message }`,
same envelope every other DB-layer failure uses). All error codes in this table are
LIVE — every one is actually enforced by the engine/session cursor state today, not
a placeholder:

| Code                        | Condition                                                                                          |
|------------------------------|------------------------------------------------------------------------------------------------------|
| `cursor_not_found`          | `FetchNext` against a cursor id the server never issued (`CancelCursor` is idempotent instead — see §4). |
| `cursor_expired`            | `FetchNext` against a cursor the server evicted after an idle-timeout. Wire-distinguishable from `cursor_not_found` via a short-lived server-side tombstone. |
| `cursor_limit_exceeded`     | `CreateCursor` rejected because the session already has `security.cursors.max_cursors_per_session` cursors open. |
| `invalid_page_size`         | CR-A3: `CreateCursor`/`FetchNext` rejected because `page_size` was `0` (would loop the client forever — `has_more` never becomes `false`) or above the server's configured `max_cursor_page_size` cap. **Rejected, never silently clamped** — a client that thinks it got `page_size` rows per page but silently got fewer would misinterpret `has_more` semantics. The cursor itself (if any) is untouched by this rejection — a bad `page_size` on one `FetchNext` call does not close or corrupt an otherwise-valid cursor. |
| `cursor_temporal_not_supported` | `CreateCursor` rejected because `query.temporal` was not `Temporal::Latest`. `AsOf`/`History` cursors are out of scope (see the status blockquote above) — rejected outright, never silently downgraded to `Latest`. |
| `cursor_page_too_large`     | CR-A5: a page's serialized size (measured once per `CreateCursor`/`FetchNext` call) exceeded `security.query_limits.max_result_size_bytes`. Rejected outright, same "never silently truncated" discipline as `invalid_page_size` — no budget is acquired and the cursor's bookmark state is left untouched by the rejection, so the client can retry with a smaller `page_size`. |
| `cursor_with_version_not_supported` | CR-B5: `CreateCursor` rejected because `query.with_version` was `true`. A cursor's every internal read (both the first page and every `FetchNext`) runs at a pinned `Temporal::AsOf` snapshot, and that read path hard-codes no per-record versions on its result — honoring `with_version` through a cursor would silently produce no versions instead of the real per-record stamps a plain read returns, breaking the optimistic-CAS contour. Rejected outright rather than silently downgraded. |

---

## 7. Out of scope for this document

- **Rust SDK ergonomic streaming** (an async `Stream` wrapper over
  create/fetch-next/cancel) — `Client::stream_cursor`, shipped.
- **TS SDK ergonomic streaming** (an async iterator wrapper) —
  `ShamirClient.streamCursor` / `Db.cursor`, shipped.
- **End-to-end tests** covering idle-timeout, cancel, and per-session cap behavior —
  shipped; see `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`
  and `crates/shamir-server/src/tests/cursor_registry_tests.rs`.

---

## 8. References

Rust types (wire DTOs):

- `crates/shamir-query-types/src/wire/db_message.rs` — `DbRequest::{CreateCursor,
  FetchNext, CancelCursor}`, `DbResponse::{CursorPage, CursorClosed}`.
- `crates/shamir-query-types/src/wire/cursor_id.rs` — `CursorId` (opaque `u64`
  newtype, `#[serde(transparent)]` — round-trips as a bare integer, not a wrapped
  object).
- `crates/shamir-query-types/src/batch/batch_error.rs` — `BatchError::{CursorNotFound,
  CursorExpired, CursorLimitExceeded, CursorTemporalNotSupported, InvalidPageSize,
  CursorPageTooLarge}`.
- `crates/shamir-query-types/src/read/query_result.rs` — `QueryResult` (reused
  verbatim as `CursorPage.page`).

Server implementation:

- `crates/shamir-server/src/db_handler/cursor_handlers.rs` — `ShamirDbHandler::{
  create_cursor, fetch_next, cancel_cursor}`, the real dispatch for all three
  requests (ACL check, registry lookup/registration, pinned-snapshot `AsOf` read,
  byte-budget/size-cap enforcement).
- `crates/shamir-server/src/cursor_registry.rs` — `CursorRegistry`/`Cursor`/
  `CursorState`, the per-session cursor table, idle-timeout reaper, and
  `cursor_expired` tombstone mechanism.

Reference client builders (produce the shapes above — see "Query construction —
builder only" in `CLAUDE.md`):

- Rust: `crates/shamir-query-builder/src/cursor.rs` — `create_cursor`, `fetch_next`,
  `cancel_cursor` free functions (cursor ops are top-level `DbRequest`s, the same
  tier as `TxBegin`/`TxCommit`, not `Batch` entries — see that module's doc comment).
- TypeScript: `crates/shamir-client-ts/src/core/builders/cursor.ts` — `createCursor`,
  `fetchNext`, `cancelCursor`, also exposed as `Batch.createCursor` / `Batch.fetchNext`
  / `Batch.cancelCursor` static helpers.
