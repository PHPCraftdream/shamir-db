# Brief: CR-A4 — keyset tie-breaker for duplicate ORDER BY values (#764)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

**This is the riskiest task in Wave A — read this brief fully before
touching code. The investigation below already answers the "can Filter
express an `_id` comparison" question so you don't have to re-derive it.**

## Problem — SILENT DATA LOSS, verified against the current tree 2026-07-23

`crates/shamir-server/src/db_handler/cursor_handlers.rs::boundary_filter`
(~lines 122-158) builds only `field > last_value` (or `<` for DESC) — a
boundary on the SOLE ORDER BY column's value, no tie-breaker. When a page
boundary falls inside a run of EQUAL ORDER BY values (e.g. `score: 10, 10,
10, 10` with `page_size: 2`), the next page's `score > 10` filter silently
skips every remaining tied row — they are gone from the read, permanently,
with no error.

## Investigation already done — read this before designing your own fix

**The `Filter` AST cannot cheaply express `_id` as a comparable field on
the cursor's read path**, and you do not need it to. `_id` only exists as
a SYNTHETIC key attached to a `QueryRecord` at result-materialization time
(`crates/shamir-query-types/src/write/inserted_record.rs`,
`get_value_owned("_id")` after the fact) — it is not a scannable "field" the
`Filter`/`FilterNode` evaluator resolves against raw record bytes during a
scan the way `boundary_filter`'s `Gt`/`Lt` on the ORDER BY field already
does. Do not try to build an OR-with-`_id`-comparison `Filter` expression —
that path does not exist today and inventing it would be a much bigger,
riskier change than this task needs.

**Instead, do the tie-break in Rust code around the existing filter/sort,
using a fact already verified in this codebase:**

1. The cursor's read always goes through `TableManager::read_as_of`
   (`crates/shamir-engine/src/table/read_temporal.rs`, ~lines 82-172, the
   ONLY path `Temporal::AsOf` uses — which is what every cursor read is).
2. `read_as_of` enumerates matched records via `self.list_stream(...)`
   (~line 82) into `matched: Vec<(RecordId, Bytes)>`, THEN applies
   `exec::apply_order_by_qv(&mut result_qv, order_by)` (~line 155).
3. `apply_order_by_qv` (`crates/shamir-engine/src/query/read/order.rs`,
   ~line 25) sorts via `idx.sort_by(...)` — **`Vec::sort_by` is a stable
   sort** (Rust std guarantee): ties preserve their PRE-sort relative
   order, which is `list_stream`'s current-enumeration order.
4. Therefore: **two separate `read_as_of` calls at the SAME pinned
   version, with the SAME WHERE/ORDER BY, and NO concurrent write between
   them, return tied rows in the SAME relative order every time.** (A
   concurrent DELETE/INSERT between calls could disturb this — that
   overlaps with the SEPARATE, already-queued task CR-B1/#767, which fixes
   `read_as_of`'s current-vs-pinned enumeration mismatch at the engine
   level. Do not expand this task into that fix; just be aware the
   guarantee you're building on here assumes no concurrent mutation until
   #767 lands, and say so in a code comment.)

## Design — inclusive boundary + skip-past-last-id

1. **Bookmark stores `(last_value, last_id)` instead of just
   `last_value`.** Extend `CursorState`/whatever struct holds `seek_key`
   (`crates/shamir-server/src/cursor_registry.rs`) to also carry the last
   returned row's `RecordId` (or its wire form — check how `_id` is
   currently extracted from a `QueryRecord` after a page is built, e.g.
   `get_value_owned("_id")`/`get_value_str("_id")` per the grep hits in
   `crates/shamir-engine/src/table/tests/keyset_seek_tests.rs` for the
   existing convention on parsing it back).
2. **`boundary_filter` builds an INCLUSIVE boundary**: `field >= last_value`
   (`Filter::Gte`) instead of `Filter::Gt` (and `Filter::Lte` instead of
   `Filter::Lt` for DESC) — `Gte`/`Lte` already exist in the `Filter` enum
   (`crates/shamir-query-types/src/filter/filter_enum.rs` ~lines 31-41).
3. **Fetch with a larger internal limit than the client's `page_size`**,
   then locate `last_id` in the returned (already stably-sorted) rows and
   slice starting IMMEDIATELY AFTER it — everything up to and including
   `last_id` is a row already returned on the previous page (or, for the
   very first tie in a run, the exact boundary row itself) and must be
   dropped; everything after is genuinely new.
4. **Bounded retry when the post-skip slice is shorter than
   `page_size`** (this happens when the tie run at the boundary is larger
   than the slack you fetched): re-issue with a LARGER internal limit
   (e.g. double it, up to a sane ceiling — reuse/relate to
   `max_cursor_page_size` from CR-A3 as the ceiling so this can't runaway)
   and repeat the skip-past-`last_id` slice, until either `page_size`
   post-skip rows are collected OR the fetch returned FEWER rows than the
   internal limit requested (meaning the underlying data is genuinely
   exhausted — no more retries needed, return what you have with
   `has_more` computed from whether you hit the true end).
5. **If `last_id` is not found in a fetch** (it was concurrently deleted,
   or your growing-limit fetch is scanning ahead but the boundary row
   itself somehow isn't there): this is the R-1/#767 territory colliding
   with this task. Document the behavior you chose (safest: treat the
   entire `>=` result as new — i.e. do NOT skip anything, on the theory
   that failing to skip in this rare edge case risks a duplicate rather
   than a silent loss, and a duplicate is a strictly less bad failure mode
   than losing a row) in a code comment citing this brief and #767.

## Also fold in (cheap, same area): pin the pagination MODE once at creation

Currently `fetch_next` re-decides keyset-vs-offset per call based on
whether `seek_key` is present. Move that decision to `create_cursor` time
(store an explicit mode — e.g. an enum field on `CursorState`,
`PaginationMode::Keyset` vs `PaginationMode::Offset` — decided ONCE from
whether the query has a simple single-column ORDER BY) so a later page
can never flip coordinate systems mid-scroll (which could duplicate or
skip rows if a projection quirk made one page's `seek_key` extraction
fail).

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- **The review's exact scenario**: 4 rows with `score: 10, 10, 10, 10`
  (identical ORDER BY value across all rows, distinguishable only by a
  different field or insertion order), `page_size: 2` — drain the cursor
  across all pages, assert the TOTAL set of returned rows is exactly the 4
  rows, each exactly once (no loss, no duplication).
- **Larger randomized-duplicates case**: e.g. 20 rows where the ORDER BY
  column only has 3-4 distinct values (heavy duplication), various
  `page_size`s — same "every row exactly once" assertion, ideally via a
  helper that collects all `_id`s across the whole cursor lifetime into a
  `HashSet` and asserts `set.len() == total_row_count` (catches both loss
  and duplication in one assertion).
- **Boundary run larger than one page**: a tie run of, say, 10 identical
  values with `page_size: 2` — proves the bounded-retry-with-growing-limit
  logic actually works when a single page_size-sized fetch can't clear the
  whole tie run in one internal attempt.
- **Pagination mode pinned**: a query whose ORDER BY field is absent from
  the projection on SOME conceptual scenario — assert the cursor still
  returns every row correctly across all pages (proves no silent
  keyset<->offset flip corrupts anything); if you can't easily construct
  a "field sometimes not in projection" scenario, at minimum test that the
  MODE decided at creation doesn't change by inspecting/asserting on
  whatever internal state you added (or via behavior alone if state isn't
  test-visible).
- **Regression guard**: existing multi-page, no-duplicates tests
  (`create_fetch_cancel_happy_path_paginates_all_rows` etc.) must stay
  green — this fix must not change behavior for the non-tied case.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Stay inside `shamir-server`
(`cursor_handlers.rs`, `cursor_registry.rs`, tests). Do NOT touch
`shamir-engine`'s `read_temporal.rs`/`order.rs` — you are relying on
their EXISTING stable-sort/enumeration behavior, not changing it (that's
CR-B1/#767's job for the enumeration-vs-DELETE issue specifically, a
separate task).
