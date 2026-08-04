# Brief — e2e gap cluster F: keyset `after_id` tie-breaker, NULLS ordering, full `PaginationInfo`

Task: #979 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, "Cluster F — Keyset `after_id` tie-breaker + NULLS ordering + pagination metadata".

## Verified mechanics — read before starting

### 1. `after_id` tie-breaker (task #537)

The keyset-seek executor (`crates/shamir-engine/src/table/read_index_scan.rs`,
`read_keyset_seek`, ~line 486-610) walks a SORTED index in value order. When
paging with `.after([value], limit)` and **no** `after_id`, the next page
seeks strictly past `value` — meaning if MULTIPLE rows share the same
ORDER-BY value and that value straddles a page boundary, any tied rows past
the first page's cutoff are **silently dropped forever** (this is the
pre-#537 bug; still the default behavior for backward compatibility when
`after_id` is omitted). Passing `after_id` (the previous page's last row's
own record id) lets the seek resume strictly past that SPECIFIC row instead
of past the bare value, so tied rows are no longer lost.

**Critical mechanic**: `_id` is injected into each row **only** on the
keyset-seek (`Pagination::After`) response path (`read_index_scan.rs`
~line 600-608 — "the ONLY read path that emits `_id`"). It is NOT present
on plain `Query.from(...).where(...)` reads. So: run a `.after(...)` query,
read `record._id` off the LAST row of the page, and echo that back as the
`afterId` param on the next `.after(key, limit, afterId)` call. Check the TS
builder signature (`crates/shamir-client-ts/src/core/builders/query.ts`
~line 318, `after(key, limit?, afterId?)`) for the exact param order/types
before writing code.

**Test to write** (extend `crates/shamir-client-ts/src/__tests__/e2e-keyset.test.ts`,
which already has the sorted-index + server setup pattern — reuse it, don't
duplicate):
1. Seed rows where MULTIPLE rows share the same `score` value that will land
   exactly at a page boundary (e.g. 5 rows with `score=50` plus distinct
   rows before/after, `PAGE=3` sized so the boundary falls mid-tie-group).
2. Page 1 → `.after([boundaryValue], PAGE)` with NO `afterId`. Page 2 →
   `.after([boundaryValue], PAGE)` again, still NO `afterId`. **Assert the
   bug reproduces**: some tied rows are missing from BOTH pages combined
   (prove today's documented `None` = "skip-all-ties" default behavior is
   real, not assumed).
3. Repeat the SAME scenario but on page 2 pass `afterId` = the `_id` of
   page 1's last row (captured from page 1's own response). **Assert all
   tied rows are now present, exactly once, across page 1 + page 2 combined**
   — this is the actual #537 fix, verified live for the first time.

### 2. `NullsOrder::{First,Last}`

Brute-force in-memory sort path (`crates/shamir-engine/src/query/read/order.rs`
~line 443-467): default when `nulls` is unset mirrors SQL-standard behavior
— ASC → nulls sort LAST, DESC → nulls sort FIRST. Explicit `.nullsFirst()`/
`.nullsLast()` (or the `nulls` param on `orderByAsc`/`orderByDesc` — check
`crates/shamir-client-ts/src/core/builders/query.ts` ~line 256-263 for the
exact TS API) overrides that default in EITHER direction.

**Test to write** (new test, home is your call — `e2e-keyset.test.ts` or a
new `e2e-nulls-order.test.ts`, check file size/convention first): seed rows
where some have a NULL value in the ORDER BY field and some don't (e.g.
`{ name: 'a', score: 10 }`, `{ name: 'b', score: null }`, `{ name: 'c',
score: 30 }`). Run 4 queries: ASC default (assert null lands LAST), ASC +
explicit `nullsFirst` (assert null lands FIRST — overrides the default),
DESC default (assert null lands FIRST), DESC + explicit `nullsLast` (assert
null lands LAST — overrides the default). This is a plain (non-indexed)
`orderBy` — no sorted index required for this part.

### 3. Full `PaginationInfo` shape

`crates/shamir-query-types/src/read/limit.rs` (~line 264-330): `total_count`
(`Option<u64>`, only when `count_total` was requested), `total_pages`
(`Option<u64>`, only computable alongside `total_count` + a page-size-bearing
pagination mode), `current_page` (`Option<u64>`), `has_next` (`bool`,
always present). Existing coverage only asserts `total_count` — `total_pages`/
`has_next`/`current_page` are never checked (js:`07`, ts:`e2e.test.ts`).

**Test to write**: run a paged query (`.limit(N)` or `.after(...)`, your
choice — pick whichever makes `current_page`/`total_pages` meaningfully
non-null per the source's own conditional logic, re-read `limit.rs` lines
284-330 to know exactly which pagination mode populates which field) with
`.countTotal(true)` (check the TS builder for the exact method name) against
a known total row count, and assert ALL FOUR `PaginationInfo` fields: exact
`total_count`, exact `total_pages` (`ceil(total/page_size)`), correct
`current_page`, and correct `has_next` (`true` when more rows remain,
`false` on the last page — test both cases with two page fetches).

## Required work

All three sub-gaps are TS-only (the JS suite has no sorted-index-driven
tests today per the existing `e2e-keyset.test.ts` precedent) — put
everything in `crates/shamir-client-ts/src/__tests__/`, extending
`e2e-keyset.test.ts` for (1) and (3) (they both need the sorted-index setup
already there), and your call whether (2) fits the same file or a new one.

Use ONLY query builders (`Query.from(...).orderByAsc(...).after(...)`,
`.countTotal(true)`, etc.) — no hand-assembled wire objects (repo-wide
CLAUDE.md rule).

## Verification

- Run the full vitest suite in `crates/shamir-client-ts` (`npx vitest run`)
  — baseline after #978 is 56 files / 1030 tests passed. Report exact counts
  before and after.
- `npx tsc --noEmit` in that package — must stay clean.
- If you touch the JS suite at all, also run `cd tests/e2e && node e2e.test.js`
  (baseline after #978: 19 files / 147 passed) and report counts.

## Scope discipline

- Do NOT touch clusters G/H (#980/#981) — low-priority misc gaps.
- Do NOT modify production Rust or the query builders themselves. If you
  find the `after_id` fix doesn't actually work as documented (tied rows
  still lost even WITH `afterId` passed), or `PaginationInfo` fields don't
  match the formula in `limit.rs`, STOP and report it as a real bug instead
  of silently adjusting the test to match broken behavior.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create test files and run read-only/test
commands.

## What to report back

List every test added and what it proves. For the `after_id` tests,
explicitly state the row counts/ids proving the bug reproduces WITHOUT
`afterId` and is fixed WITH it. For `PaginationInfo`, state the exact
numeric values asserted for each of the 4 fields. Give exact test-run
output with real pass/fail counts.
