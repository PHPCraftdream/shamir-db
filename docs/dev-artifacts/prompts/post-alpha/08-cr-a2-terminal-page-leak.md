# Brief: CR-A2 — don't register an already-exhausted cursor (#761)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — RESOURCE LEAK, verified against the current tree 2026-07-23

`crates/shamir-server/src/db_handler/cursor_handlers.rs::create_cursor`
computes `has_more` (~line 297), builds the `Cursor` + its pinned
`SnapshotGuard`, sets `state.exhausted = !has_more`, then calls
`self.cursor_registry.register(...)` **unconditionally** (~lines 330-337)
regardless of whether `has_more` is `true` or `false`.

When the ENTIRE result fits on the first page — an empty table, a result
shorter than `page_size`, an `EXPLAIN` query, or any query small enough —
`has_more` is `false`. Neither SDK ever calls `FetchNext` in that case
(both `CursorStream` (Rust) and `CursorIterator` (TS) stop as soon as
`has_more == false`). The cursor's pinned MVCC `SnapshotGuard` and its
per-session registry slot are therefore held for no reason until the
background idle-timeout reaper reclaims them (default 60s) — a session
issuing 16 (the default `max_cursors_per_session`) short single-page
cursor queries in a row can hit `cursor_limit_exceeded` even though every
one of those cursors is logically already "done."

## Fix

When `has_more` is `false` after the first page, **do not call
`self.cursor_registry.register(...)` at all.** Return
`DbResponse::CursorPage { cursor_id: CursorId(cursor_id), page, has_more: false }`
directly and let the local `cursor` value (and the `SnapshotGuard` it
owns) drop at the end of the function — RAII releases the MVCC pin
immediately, no reaper wait needed, and the per-session cap counter is
never touched by an exhausted cursor.

The `cursor_id` returned can stay exactly as-is (still minted via
`self.next_cursor_id()` before this branch) — it is harmless to hand the
client an id that was never registered, since:
- Neither SDK uses the id again once `has_more == false` (both already
  stop iterating).
- If a client DID send a `FetchNext` against this never-registered id, it
  correctly falls through to `CursorRegistry::get_owned`'s existing
  `CursorNotFound` path (not a panic, not `CursorExpired` — genuinely
  never existed, which is the accurate answer here).
- `CancelCursor` against this id is already idempotent-safe
  (`CursorRegistry::get_owned` fails silently, `cancel_cursor` handler
  returns `CursorClosed` regardless per the existing "idempotent close"
  contract).

Where exactly: right at the `match self.cursor_registry.register(...)`
call — restructure so the `has_more == false` case short-circuits BEFORE
attempting registration, e.g.:

```rust
if !has_more {
    // Entire result fit on the first page — nothing to register. Drop
    // `cursor` (and its SnapshotGuard) here via RAII instead of parking
    // it in the registry for the reaper to eventually reclaim.
    return DbResponse::CursorPage {
        cursor_id: CursorId(cursor_id),
        page,
        has_more: false,
    };
}

match self.cursor_registry.register(
    cursor_id,
    session.session_id,
    cursor,
    self.cursor_limits.max_cursors_per_session as u32,
) {
    // ...unchanged...
}
```

(Adjust exactly to the surrounding code's actual shape — read the current
function body before editing, it may have shifted slightly since this
brief was written.)

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`
(mirror the existing fixture style — `build_handler_with_rows`,
`alice_session`, `send`/`create_cursor_req` helpers already there):

- **Rapid-fire short cursors don't exhaust the per-session cap**: with
  `max_cursors_per_session` set low (e.g. 2 via `CursorLimitsCap`), issue
  MORE than that many single-page `CreateCursor` calls in a row on the
  SAME session (each returning `has_more: false` — e.g. query a table with
  fewer rows than `page_size`), and assert every single one succeeds
  (`DbResponse::CursorPage`), not `cursor_limit_exceeded`.
- **Registry stays empty**: after each such single-page `CreateCursor`,
  assert `handler.cursor_registry().len() == 0` (or the count doesn't
  increase) — proves nothing was actually registered.
- **A `FetchNext` against the returned (never-registered) id** gets a
  clean `cursor_not_found` response, not a panic — exercise this
  explicitly even though it should already work through the existing
  registry-lookup path.
- **Empty table**: `CreateCursor` over zero rows returns
  `CursorPage{page: {records: []}, has_more: false}` and is not
  registered (covers the review's explicit "empty table" case).
- **Multi-page case is unaffected**: keep/extend an existing test proving
  a query that DOES span multiple pages still registers normally on its
  first (non-exhausted) page — a regression guard so this fix doesn't
  accidentally skip registration when `has_more` is actually `true`.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Stay inside
`crates/shamir-server/src/db_handler/cursor_handlers.rs` and its test
file. This touches the SAME `create_cursor` function CR-A1 just modified
(already committed, `b860765c`) — re-read the current file state before
editing, do not assume the line numbers from CR-A1's own brief still
apply verbatim.
