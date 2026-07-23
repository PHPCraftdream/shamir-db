# Brief: CR-C2 — cursor test-coverage gaps (#777)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

**This is a test-only task — no production code changes expected.** All
new tests go into `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`,
matching the existing fixture style in that file exactly (`build_handler_with_rows`,
`create_cursor_req`, `fetch_next_req`, `fetch_next_default_req`, `send`,
`alice_session`). If while writing a test you discover an ACTUAL bug (not
just a coverage gap), stop, document exactly what you found in your final
report, and do NOT attempt to fix it yourself — that's a separate task's
scope; just report it clearly so the orchestrator can triage it.

## Context — why these gaps matter

`crates/shamir-server/src/db_handler/cursor_handlers.rs::pagination_mode_for_query`
(~lines 391-396) picks `PaginationMode::Keyset` ONLY when the query has
EXACTLY one `ORDER BY` item on a single (non-nested) field:

```rust
fn pagination_mode_for_query(query: &ReadQuery) -> PaginationMode {
    match &query.order_by {
        Some(ob) if ob.items.len() == 1 && ob.items[0].field.len() == 1 => PaginationMode::Keyset,
        _ => PaginationMode::Offset,
    }
}
```

Every other shape — NO `ORDER BY` at all, multi-column `ORDER BY`, a
nested field path (`field.len() != 1`) — falls back to
`PaginationMode::Offset` (the row-count bookmark path,
`fetch_next`'s `_ =>` match arm). Despite this being the silent fallback
for FOUR distinct real-world query shapes, EVERY existing cursor handler
test uses a single-column `OrderBy::asc(...)`/`OrderBy::desc(...)` — the
offset path has ZERO direct test coverage today (confirm this yourself by
grepping the existing test file for `OrderBy::` — every single hit is a
one-column call). `order_by_field_value` (~lines 406-412) also returns
`None` (triggering the SAME offset-mode-equivalent fallback behavior
within an otherwise-keyset-eligible query) whenever the ORDER BY field
isn't present in the record — i.e. an ORDER BY on a field excluded from
the `SELECT` projection — also untested.

## Tests to add

Add all of these to `cursor_handler_tests.rs`, near the existing
pagination-mode-related tests (search for `keyset_no_ties_regression`/
`CR-A4`-tagged tests for a natural location) — one test function per
bullet below, each with a clear doc comment naming which gap it closes:

1. **No-`ORDER BY` full pagination.** A cursor with NO `order_by` at all,
   multiple pages (e.g. 7 rows, `page_size: 3` → pages of 3, 3, 1), draining
   the whole cursor and asserting every original row (by `id` or `v`)
   appears EXACTLY ONCE across all pages, at a single pinned snapshot. This
   is the most basic no-`ORDER-BY` coverage this codebase has never had.

2. **Multi-column `ORDER BY` falls back to the offset bookmark, still
   correct.** Build a query with TWO `OrderBy` items (check `OrderBy`'s
   builder API — likely something like `OrderBy::new(vec![...])` or
   chained `.then_by(...)`; find the actual constructor in
   `shamir_query_types::read::OrderBy` before writing this, don't guess
   the API) — e.g. order by `v` ascending, then `id` ascending as a
   tiebreaker (with this fixture's data, `v` is already unique per row so
   the second column is inert, but the POINT is that
   `pagination_mode_for_query` sees `items.len() != 1` and routes to
   `PaginationMode::Offset` regardless of whether the extra column
   matters semantically — assert every row appears exactly once across
   multiple pages, same shape as test 1).

3. **`page_size` varying across `FetchNext` calls.** `CreateCursor` with
   one `page_size` (e.g. 2), then a SEQUENCE of `FetchNext` calls each
   requesting a DIFFERENT explicit `page_size` (e.g. 5, then 1, then
   whatever's left) — assert the total set of rows across every page is
   correct (every row exactly once, no duplicates, no losses) and that
   each individual page's row count matches what was actually requested
   (capped by remaining data). This is a documented capability
   (`FetchNext`'s `page_size` may differ per call — see `CURSORS.md` §3)
   with no existing test exercising an actual size CHANGE mid-lifecycle
   (every existing test uses the SAME size for every call).

4. **Concurrent `FetchNext` on the same cursor.** Use `tokio::join!` (or
   `tokio::spawn` + `.await` both) to fire TWO `FetchNext` calls against
   the SAME `cursor_id` at (as close to) the same time, from the same
   session. `fetch_next`'s `state = cursor.state().lock().await` (a
   `tokio::sync::Mutex`, check the exact type in `cursor_registry.rs`)
   should serialize the two calls' bookmark advances — investigate what
   the ACTUALLY-DEFINED behavior is (does the loser see the winner's
   advanced bookmark and get the NEXT page in sequence — i.e. two
   different, non-overlapping pages — or could the two race in some
   OTHER well-defined way?) by reading `fetch_next`'s locking code
   yourself, then write a test asserting THAT observed behavior
   explicitly, with a comment stating what property you're relying on
   (e.g. "the mutex serializes advances, so two concurrent FetchNext
   calls always produce two DISJOINT, correctly-sequenced pages — never
   overlapping, never skipping"). Assert the COMBINED result of both
   calls has no duplicate rows and no lost rows relative to what should
   have been returned by that point in the cursor's lifecycle.

5. **Unprojected-seek-field fallback.** A query with `ORDER BY v` (single
   column, otherwise keyset-eligible) but whose `SELECT` projection
   EXCLUDES `v` — construct via `ReadQuery::new("items").select(Select::fields(["id"])).order_by(OrderBy::asc("v"))`
   (check `Select`/`OrderBy`'s exact import path — this exact
   `.select(Select::fields([...]))` pattern already appears in
   `crates/shamir-engine/src/table/tests/asof_read_tests.rs`, e.g. line
   104, as precedent for the builder shape; the cursor tests file may
   need a new `use shamir_query_types::read::Select;` import). This
   triggers `order_by_field_value`'s `None` return (the field isn't in
   the projected record), which the code treats as "can't refresh a
   keyset bookmark from this page" per its own doc comment
   (~lines 398-401) — verify (by reading the actual `fetch_next`/
   `create_cursor` code paths that call `order_by_field_value`) exactly
   what fallback behavior results or is currently DOCUMENTED to result
   (does it degrade to something ELSE, or is it genuinely broken/
   untested territory?), and write a test proving it either (a) still
   pages every row correctly via whatever the actual fallback is, or (b)
   if you discover it does NOT correctly page every row, this is a REAL
   BUG — do not fix it, write the test capturing the CURRENT (possibly
   broken) behavior with a clear `// KNOWN GAP:` comment, and flag it
   prominently in your final report so the orchestrator can create a
   follow-up task rather than silently asserting broken behavior as if
   it were correct.

**Explicitly excluded from this task** (do not add): delete-mid-scroll
coverage — already covered by CR-B1's own tests
(`cursor_still_returns_a_row_deleted_mid_scroll` etc. in this same file).
Don't duplicate that coverage.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Stay inside
`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs` — no
production code changes unless you discover and clearly flag a real bug
per test 5's instructions above (and even then, do NOT fix it in this
task — just flag it).
