# Brief: CR-B4 — `has_more` peek-ahead, no spurious empty last page (#770)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — documented-lie heuristic, verified against the current tree 2026-07-23

Two of the three places `has_more` is computed in
`crates/shamir-server/src/db_handler/cursor_handlers.rs` use the heuristic
`fetched_count >= page_size`, which is WRONG whenever the true remaining
result set is an EXACT multiple of `page_size`: the true last page reports
`has_more: true`, and the client (both the Rust `CursorStream` and the TS
`CursorIterator`, which stop only on `has_more == false`) performs one
spurious extra round-trip and gets back an empty page. `DbResponse::CursorPage.has_more`'s
own doc comment (`crates/shamir-query-types/src/wire/db_message.rs`, search
for `has_more` — cite the exact line when you find it) promises "`true` if
a further `FetchNext` will return **at least one more record**" — the
current heuristic is a documented lie on an exact-multiple result set.

**The two buggy call sites:**

1. `create_cursor`'s first page
   (`cursor_handlers.rs`, ~line 755-794): fetches EXACTLY `page_size` rows
   (`Pagination::LimitOffset { limit: Some(page_size as u64), offset: 0 }`)
   and computes `has_more = page.records.len() as u64 >= page_size as
   u64` (~line 794) — no peek-ahead at all.
2. `fetch_next`'s OFFSET-bookmark branch (the `_ =>` arm at ~line 1045-1069,
   used when there's no simple single-column `ORDER BY` to keyset-seek on):
   fetches exactly `effective_page_size` rows and computes `has_more =
   fetched.records.len() as u64 >= effective_page_size as u64` (~line
   1064) — same flawed heuristic.

**Already correct — do NOT touch:** `fetch_next`'s KEYSET-bookmark branch
(`fetch_keyset_page`, ~lines 502-638, called from the `(PaginationMode::Keyset,
Some(seek_key))` arm at ~line 1018-1044) ALREADY does a proper peek-ahead —
CR-A4 (#764) built its internal-limit-plus-one fetch for tie-breaking
correctness, and `has_more = consumed_from_front < page.records.len()`
(~lines 617, 630) already correctly distinguishes "there's a real row
beyond what we're returning" from "that's genuinely everything." Verify
this yourself by re-reading `fetch_keyset_page`/`finish_keyset_page`
before touching anything nearby — this task's scope is ONLY the two
buggy sites listed above.

## Fix — peek-ahead by one row, apply at both buggy sites

For each of the two sites: change the internal fetch's `limit` from the
client-visible page size to `page_size + 1` (or `effective_page_size + 1`
for site 2) — a saturating/checked add so a page size already at
`max_cursor_page_size` (validated by CR-A3) cannot overflow `u32` (the
value flows into a `u64` pagination limit field already, so widening to
`u64` before the `+1` sidesteps any real overflow risk — check the exact
field type in `Pagination::LimitOffset` before deciding whether a plain
`+1` is already safe or needs an explicit `saturating_add`/`as u64` cast
first).

Then:
- If `page_size + 1` rows came back: return only the first `page_size` of
  them to the client, and set `has_more = true` (this is now
  UNCONDITIONALLY correct — a peek row existing proves there's at least
  one more).
- Else (fewer than `page_size + 1` rows came back, i.e. `<= page_size`):
  return everything that came back, `has_more = false`.

**Interactions to get right at each site:**

1. **Bookmark must come from the LAST RETURNED row, not the trimmed peek
   row.** At site 1 (`create_cursor`), the seek_key/tie_skip computation
   (~lines 795-821) currently derives from `page.records.last()` — after
   this fix, that must be the last row of the TRIMMED (`page_size`-length)
   slice, not the raw fetch's last row (which could be the peek row). Site
   2 (offset branch) doesn't build a seek_key at all (`new_seek_key = None`
   unconditionally, offset-mode has no keyset bookmark) — just make sure
   `new_offset`'s arithmetic (next point) is right.
2. **Offset-bookmark arithmetic advances by the RETURNED count, not the
   fetched count.** Site 2's `new_offset = state.offset +
   fetched.records.len() as u64` (~line 1065) must become `state.offset +
   <returned count>` (i.e. `page_size` when the peek row was present and
   trimmed off, or the raw fetched count when it wasn't) — advancing by
   the peek-inflated count would skip a row on the NEXT page.
3. **Composes with CR-A2's terminal-page fix.** An exact-multiple final
   page at `create_cursor` time must now correctly compute `has_more =
   false` and hit the existing "not registered, `SnapshotGuard` drops
   immediately" branch (~lines 826-846) — this should fall out naturally
   once `has_more` itself is computed correctly; no changes needed to that
   branch's own logic, just make sure it now triggers on the RIGHT
   condition. Symmetrically, `fetch_next`'s offset branch already removes
   the cursor from the registry when `!has_more` (~lines 1092-1094) — same
   "should just work once has_more is correct" expectation.
4. **`CursorLimitsCap`/`max_cursor_page_size` interaction.** The `+1`
   internal fetch limit must NOT be validated against
   `max_cursor_page_size` as if it were a client-visible page size — it's
   an internal implementation detail one row larger than the (already
   separately validated) client-visible size. Don't run the `page_size ==
   0 || page_size > max_page_size` check (CR-A3, already happens earlier at
   both sites, unaffected by this task) against the `+1`'d internal value.
5. **`stats.records_returned` / any other page metadata derived from
   `page.records.len()`.** After trimming the peek row off, make sure any
   stats/count field on the returned `QueryResult` reflects the TRIMMED
   count, not the raw fetch's count (mirrors how `finish_keyset_page`
   already corrects `stats.records_returned` after its own trim, ~line
   663-665 — use that as a reference pattern for site 1 and site 2's
   post-trim cleanup).

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- **Exact-multiple result, `create_cursor`**: a result set of exactly
  `page_size` rows (e.g. 2 rows, `page_size: 2`) — `CreateCursor`'s
  response must report `has_more: false` immediately (single page, cursor
  never registered — reuse/extend the existing CR-A2 "exhausted first page
  is not registered" test pattern as a template).
- **Exact-multiple result, `fetch_next` offset path**: a query with NO
  simple single-column `ORDER BY` (so it takes the offset-bookmark branch —
  check how existing tests force this branch, e.g. search for how the
  offset-mode tests in this file already construct such a query) with a
  total row count that's an exact multiple of `page_size` across multiple
  pages — the TRUE last page must report `has_more: false` with NO
  subsequent empty-page round-trip needed. Assert by draining the cursor
  and counting exactly how many `FetchNext` calls were needed — it must
  match the expected page count exactly, not one more.
- **Non-multiple results unchanged**: an existing non-exact-multiple test
  (or a new one) proving the fix doesn't change behavior when there
  genuinely IS a partial final page.
- **Bookmark correctness across the trimmed peek row**: the row that gets
  peeked-and-trimmed on one page must reappear as the FIRST row of the
  NEXT page, exactly once (not skipped, not duplicated) — for BOTH the
  `create_cursor`-first-page-then-first-`FetchNext` transition and a
  multi-page offset-mode drain.
- **Regression guard**: existing cursor tests
  (`create_fetch_cancel_happy_path_paginates_all_rows`,
  `keyset_no_ties_regression_every_row_returned_once_in_order`, the CR-A4
  tie-breaker tests, the CR-A2 terminal-page test) must all stay green.

## SDK-side check (do not skip)

Search the Rust SDK's cursor stream tests
(`crates/shamir-client/src/tests/cursor_stream_tests.rs`) and the TS e2e
cursor tests (`crates/shamir-client-ts/src/__tests__/e2e-cursors.test.ts`,
`e2e-cursor-lifecycle.test.ts`) for any assertion that currently expects
(or tolerates) an extra empty final page, or asserts an exact round-trip
COUNT that assumed the old off-by-one behavior. Update any such assertion
to the now-correct expectation (fewer round-trips for an exact-multiple
result). If nothing asserts on round-trip counts today, no changes are
needed there — say so explicitly in your report rather than silently
skipping the check.

## Gate

```
cargo fmt -p shamir-server -p shamir-client -- --check
cargo clippy -p shamir-server -p shamir-client --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -p shamir-client --full
```

If you changed any TS e2e test file per the SDK-side check above, also run
(from `crates/shamir-client-ts/`):
```
npm run typecheck
npm test
```
and verify against a rebuilt release `shamir-server` binary if the changed
test is an e2e test that spawns a real server (check the test file's setup
— if it's an e2e test, the release binary must be freshly built with THIS
task's changes before the test result means anything; a stale binary would
silently validate against the OLD behavior).

All must pass before returning. Primary code area: `shamir-server`
(`cursor_handlers.rs`, tests). Do NOT touch `fetch_keyset_page`/
`finish_keyset_page` (already correct, CR-A4's territory) or the byte-budget
wiring (CR-B2's territory) or the `Option<u32>` page_size plumbing
(CR-B3's territory, already landed) — this task only fixes the internal
fetch-limit and `has_more` computation at the two named sites.
