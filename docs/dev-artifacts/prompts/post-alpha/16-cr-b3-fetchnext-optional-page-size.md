# Brief: CR-B3 — `FetchNext.page_size` becomes `Option<u32>` (#769)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations. For the TS side,
run `npm run typecheck` / the test command in the foreground too — do not
background it.

## Problem — dead field / documentation mismatch, verified against the current tree 2026-07-23

`DbRequest::FetchNext { page_size: u32, .. }`
(`crates/shamir-query-types/src/wire/db_message.rs:239-244`) is a
MANDATORY wire field — there is no way to omit it. Yet `CreateCursor`'s own
doc comment (`db_message.rs:220-233`) and `CURSORS.md` both promise
`page_size` is "the default for subsequent `FetchNext` calls that omit an
override." Correspondingly, `Cursor::default_page_size`
(`crates/shamir-server/src/cursor_registry.rs:187,211,227`) is stored and
exposed via a public accessor (`cursor_registry.rs:247-248`) but NEVER
consumed anywhere — `fetch_next`
(`crates/shamir-server/src/db_handler/cursor_handlers.rs:889-907`) uses its
`page_size: u32` parameter unconditionally, with no fallback path. The
field is provably dead code documenting a contract the wire format cannot
actually fulfill.

## Fix — make the wire field `Option<u32>`, wire up the stored default

### 1. Wire type (`shamir-query-types`)

In `crates/shamir-query-types/src/wire/db_message.rs`, change
`DbRequest::FetchNext.page_size` from `u32` to `Option<u32>` with
`#[serde(default)]` (so an omitted field on the wire decodes to `None`,
matching msgpack's normal "missing key" semantics for an `Option`). Update
the field's doc comment to state the real contract: `Some(n)` requests `n`
records for this page (client-controlled per-call backpressure, unchanged
from today); `None` falls back to the cursor's stored
`CreateCursor`-time default. Add/update serde round-trip tests (find the
existing wire round-trip test module for `DbRequest` cursor variants,
likely under `crates/shamir-query-types/src/wire/tests/` or similar —
search for `FetchNext` in existing test files) covering BOTH `Some(n)` and
`None` encoding/decoding.

### 2. Server dispatch (`shamir-server`)

`crates/shamir-server/src/db_handler/handler.rs`'s `DbRequest::FetchNext {
cursor_id, page_size }` dispatch arm (search for it, calls
`self.fetch_next(session, cursor_id, page_size).await`) now passes
`Option<u32>` through unchanged — the resolution to a concrete `u32`
happens inside `fetch_next` itself (next point), not at the dispatch
layer.

`crates/shamir-server/src/db_handler/cursor_handlers.rs`'s `fetch_next`
(~lines 889-925): change the `page_size: u32` parameter to `page_size:
Option<u32>`. Restructure the validation/lookup order carefully — CR-A3's
existing comment (~lines 895-900) explains WHY `page_size` is validated
BEFORE the registry lookup (avoids a wasted registry hit AND avoids ever
reaching the `has_more` infinite-loop computation for a malformed
request). Preserve that property for the `Some(n)` case (validate `n`
against `0`/`max_cursor_page_size` immediately, exactly as today, still
before the registry lookup). For the `None` case, there is nothing to
validate yet — you don't know the cursor's stored default until AFTER the
registry lookup succeeds, so: skip the pre-lookup validation when
`page_size` is `None`, do the registry lookup, resolve the effective page
size via `cursor.default_page_size()`, THEN validate that resolved value
against `0`/`max_cursor_page_size` (defense-in-depth — the default was
already validated once at `CreateCursor` time via CR-A3, so this should
always pass in practice, but validate again rather than silently trusting
stored state; if this validation somehow fails, treat it as a normal
`InvalidPageSize` rejection, same shape as the `Some(n)` case, and do NOT
mutate/close the cursor for it — same "a bad page_size on one call must
not corrupt the cursor" invariant CR-A3 already established). Below the
validation point, replace every remaining use of `page_size` in the
function body with the resolved `effective_page_size: u32`.

### 3. Rust query builder (`shamir-query-builder`)

`crates/shamir-query-builder/src/cursor.rs`'s `fetch_next` (~lines 60-68):
change its signature to accept `page_size: Option<u32>` (matching the wire
field's new type — this crate's own rule is "produce exactly the wire
shape, nothing more," so mirror the type change directly rather than
inventing a new API surface). Update the doc comment. Update
`crates/shamir-query-builder/src/cursor/tests.rs` (existing calls at lines
~51 and ~66 pass a bare `u32` today — update to `Some(25)` etc., and add a
new test for the `None` case).

### 4. Rust SDK (`shamir-client`)

`crates/shamir-client/src/cursor_stream.rs:142` calls `fetch_next(cursor_id,
page_size)` where `page_size` is the stream's own internal page-size
state (an explicit value the stream always tracks) — update the call site
to `fetch_next(cursor_id, Some(page_size))`. This is a mechanical
call-site fix; `CursorStream`'s own public API/behavior does not change
(it always has a concrete page size to pass, per the task's own note that
"SDK internals can keep passing explicit sizes — no behavior change
required there"). Update
`crates/shamir-client/src/tests/cursor_stream_tests.rs:168`'s direct
builder call similarly (`Some(2)`).

### 5. TS query builder + types (`shamir-client-ts`)

`crates/shamir-client-ts/src/core/types/cursor.ts`'s `FetchNextRequest`
interface (~line 44-48): change `page_size: number` to `page_size?:
number` (TS's natural "optional field, omit or send `undefined`"
counterpart to Rust's `Option` + `#[serde(default)]` — check that the
serialization layer this SDK uses (likely a shared msgpack encode step)
already drops `undefined` fields rather than encoding them as `null`; if
it doesn't, you may need `page_size: number | undefined` plus an explicit
strip step, but prefer the natural optional-field approach if the existing
encode path already supports it elsewhere in this file's sibling types —
check how other optional wire fields, e.g. `CreateCursorRequest.query_version?:
number`, are handled for precedent).

`crates/shamir-client-ts/src/core/builders/cursor.ts`'s `fetchNext`
(~lines 61-72): change `pageSize: number` to `pageSize?: number`, and only
include `page_size` in the returned object when `pageSize !== undefined`
(don't emit `page_size: undefined` into the object — that can serialize
differently from a truly absent key depending on the encode layer; check
how `createCursor`'s already-optional handling, if any, or other
sibling builders in this file handle an absent-vs-`undefined` field and
mirror that convention).

`crates/shamir-client-ts/src/core/builders/batch.ts:428`'s
`Batch.fetchNext(cursorId, pageSize: number)` static (a convenience
re-export/wrapper, check its exact relationship to `builders/cursor.ts`'s
`fetchNext`) — same optional-parameter treatment.

`crates/shamir-client-ts/src/core/cursor-iterator.ts:124` calls
`fetchNext(this._cursorId, this.pageSize)` where `this.pageSize` is the
iterator's own tracked page size (always a concrete number) — this call
site needs no behavioral change, just confirm it still type-checks against
the new optional parameter (passing a concrete `number` into an optional
parameter slot is always valid).

Update `crates/shamir-client-ts/src/core/builders/__tests__/cursor.test.ts`
(existing calls at lines ~34/~65 pass `fetchNext(7, 25)` / a numeric
`pageSize` — these stay valid since the parameter is still assignable from
a plain number) and ADD a new test for the omitted-`pageSize` case (assert
the built request either has no `page_size` key, or omits it correctly
per whatever convention you land on in point 5).

### 6. Docs

`docs/guide-docs/client-server-protocol-spec/CURSORS.md`'s `FetchNext`
section (§3, the field table and its surrounding prose): update to
describe `page_size` as OPTIONAL now, with the fallback-to-`CreateCursor`
default behavior finally being real (not just documented-but-unenforceable
as it was before this fix). Update the msgpack example if it currently
shows `page_size` as always-present-looking.

## Tests (TDD — write failing tests first)

- **Wire round-trip** (`shamir-query-types`): `FetchNext` with `Some(n)`
  and with `None` both encode/decode correctly.
- **Server behavior** (`shamir-server`, `cursor_handler_tests.rs`): a
  `FetchNext` that OMITS `page_size` (sends `None`) uses the
  `CreateCursor`-time default page size — behavioral test through the real
  handler (create a cursor with a specific `page_size`, then send
  `FetchNext` with `page_size: None`, assert the returned page's record
  count matches the CREATE-time default, not some other value). Also keep
  a regression test proving an explicit `Some(n)` on `FetchNext` still
  overrides the default (the existing per-call-backpressure behavior).
  Also test the invalid-default-defense-in-depth path is at least
  reachable/sane (doesn't need to force an actual invalid stored state —
  a code-review-level sanity check plus the existing CR-A3 coverage for
  the `Some(0)`/`Some(too-large)` cases is enough; don't over-engineer a
  test for an invariant-violation state that should be unreachable in
  practice).
- **Builders produce the right shapes** on both sides (Rust
  `shamir-query-builder`/`shamir-client`, TS `shamir-client-ts`) — omitted
  vs. explicit page size.

## Gate

```
cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-server -p shamir-client -- --check
cargo clippy -p shamir-query-types -p shamir-query-builder -p shamir-server -p shamir-client --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-server -p shamir-client --full
```

For the TS side, run from `crates/shamir-client-ts/`:
```
npm run typecheck
npm test
```
(use whatever the actual package.json script names are — check
`crates/shamir-client-ts/package.json` if these exact names don't exist).

All must pass before returning. Do NOT touch cursor pagination/tie-breaker
internals (CR-A4's territory), the byte-budget wiring (CR-B2's territory),
or ACL/temporal-rejection logic (CR-A1/CR-B5's territory) — this task only
changes the TYPE and resolution of `page_size` on `FetchNext`, nothing
else about cursor behavior.
