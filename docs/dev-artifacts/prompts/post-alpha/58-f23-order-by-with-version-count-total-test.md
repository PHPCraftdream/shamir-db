# Brief for F-23 (#816, P2) — test the `ORDER BY + with_version + count_total` triple combination

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-6 (#796) and F-7 (#797), both already landed, each modified the same
gate independently: `use_topk` (the top-K heap fast path for `ORDER BY +
LIMIT`) is excluded whenever `count_total` is requested (F-6) AND whenever
`with_version` is requested (F-7) — both push the query onto the full-sort
path instead (`apply_order_by_qv_with_ids` threads `RecordId`s through the
sort for F-7; `apply_pagination` computes `total_count` for F-6). The
post-wave `/crush` review (NF-5,
`docs/dev-artifacts/research/2026-07-26-wave-f-post-review-crush/REPORT.md`)
confirmed the CODE already handles both flags together correctly (verified
by reading `use_topk`'s gate condition and the full-sort path) but found
**no test exercises the combination of all three: `ORDER BY` + `LIMIT` +
`with_version: true` + `count_total: true` at once** — a pure test-gap,
not a bug, but worth pinning against a future refactor of the `use_topk`
gate accidentally re-introducing a case where one flag's correctness
(id-threading for `with_version`, or the `total_count` computation) breaks
because the other flag's code path took a shortcut it shouldn't have.

## What to add

Add ONE new test function to
`crates/shamir-engine/src/table/tests/with_version_order_by_tests.rs`,
following the exact conventions the existing 5 tests in that file already
use (read the whole file first — it's short, ~370 lines):

- Reuse `make_plain_mvcc_table()` and `insert_scored()` (existing helpers
  in this file).
- Build a `ReadQuery` with `.order_by(OrderBy::asc("score"))`, a `.limit()`
  (small enough to force pagination, e.g. 2 of 5 inserted rows — mirror
  Test 1's own pagination sub-case, which already proves the `with_version`
  + pagination combination alone), `with_version = true`, AND
  `count_total(true)` (the builder method, `crates/shamir-query-types/src/
  read/read_query.rs` ~line 140-142) — check whether `count_total` is a
  `ReadQuery` builder method (chainable) or a field set directly like
  `with_version` in the existing tests, and match whichever style this
  file's existing tests already use for `with_version` (direct field
  assignment, `q.with_version = true;`).
- Assert ALL THREE things line up correctly, together, in the SAME
  response:
  1. `res.records` are correctly sorted and correctly paginated (mirror
     Test 1's assertions for the equivalent case without `count_total`).
  2. `res.versions` is `Some(...)`, index-aligned with the paginated
     `records` — same style of assertion as Test 1's pagination sub-case
     (compare against ground-truth versions read via `mvcc.version_of`).
  3. `res.pagination` (or wherever `total_count` surfaces on `QueryResult`
     — check the exact field path, likely `res.pagination.as_ref().and_then(|p|
     p.total_count)`) is `Some(5)` (the TRUE total row count across the
     whole table, not just the returned page size) — proving `count_total`
     and `with_version` did not interfere with each other on the shared
     full-sort code path.
- Use a clear doc comment above the test (matching this file's existing
  per-test doc-comment style) explaining this pins the F-6∩F-7 interaction
  the post-wave review flagged as untested (cite F-23/#816 and NF-5).

## Constraints

- This is a NEW test only — do NOT modify any of the 5 existing tests in
  this file, and do NOT touch any production code (`table.rs`,
  `read_exec.rs`, `apply_pagination`, `apply_order_by_qv_with_ids`, etc.) —
  the review confirmed the code path is already correct; this task only
  adds coverage.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- with_version_order_by
```
