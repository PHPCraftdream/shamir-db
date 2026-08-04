# Brief — e2e gap cluster D: covering `include` index, composite index, DROP `if_exists`

Task: #977 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, "Cluster D — Index2 DDL extras: covering `include`, composite, drop `if_exists`".

## Verified scope (read before starting)

Three independent, unrelated gaps — all zero live-server coverage today
(confirmed via grep across `tests/e2e/tests/*.js` and
`crates/shamir-client-ts/src/__tests__/*.ts`):

1. **Covering (`include`) sorted index.** `CreateIndexOp.include: Vec<Vec<String>>`
   (`crates/shamir-query-types/src/admin/types/index_ops.rs` ~line 72-76) is
   only valid on `sorted: true` indexes (server rejects otherwise —
   `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` ~line 389).
   The TS builder already supports it: `ddl.createIndex(name, table, fields,
   { sorted: true, include: [['field2'], ['field3']] })`
   (`crates/shamir-client-ts/src/core/builders/ddl.ts` ~line 157-210).
   When a range/order query is served entirely from the covering index (no
   data-store fetch needed), `QueryResult.stats.index_used` reports
   `"sorted_idx_<index_name>_covering"` — this is the ONLY observable proof
   the covering path actually ran (`crates/shamir-engine/src/table/read_index_scan.rs`
   ~line 172-204). Write a test: create a sorted index on field A with
   `include: [['B']]`, insert rows, run a range query (`between`/`gt`/order-by
   on A) that also projects/reads field B, and assert `stats.index_used`
   ends in `_covering`. Also add a negative-shape check: `include` on a
   NON-sorted index must be REJECTED by the server (assert the error, per
   the source line above) — cheap and closes a real DDL-validation gap.

2. **Composite (multi-field) index — regular only, NOT sorted.** `fields:
   Vec<Vec<String>>`'s outer dimension is the list of columns; a REGULAR
   (non-sorted, non-unique) index accepts multiple columns as one composite
   key (`table.create_index(name, paths)` in
   `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` ~line 498).
   **Sorted indexes explicitly reject multi-field** today — "composite TBD"
   (`admin_table_index.rs` ~line 392-397, `if op.fields.len() != 1`). Write a
   test: `ddl.createIndex(name, table, [['a'], ['b']])` (regular, no
   `sorted`), insert rows with varying `(a,b)` combos, query with an `And`
   filter on both `a` and `b` equality, and assert the composite index is
   actually used (`stats.index_used`) and returns only exact `(a,b)` matches
   (not just `a` matches). Also add a negative-shape check: the SAME
   multi-field `fields` array with `sorted: true` must be REJECTED with the
   "composite TBD" error — proves the current limitation is enforced, not
   silently mishandled.

3. **`drop_index` with `if_exists: true`.** `DropIndexOp.if_exists`
   (`index_ops.rs` ~line 124-128) makes a missing-index drop a silent no-op
   returning `{"existed": false}` instead of erroring. Check
   `crates/shamir-client-ts/src/core/builders/ddl.ts` for the exact
   `dropIndex(...)` builder signature/option name before writing — do not
   guess. Write a test mirroring the existing `if_not_exists`-style pattern
   already used elsewhere in the suite (see `G3-dropUser` in
   `crates/shamir-client-ts/src/__tests__/e2e-permissions.test.ts` ~line 659
   for the idiom, though that's `dropUser` not `dropIndex` — same shape,
   different op): drop a real index (assert `existed: true` or whatever the
   real response field is), then drop-again WITHOUT `if_exists` (assert it
   errors), then drop-again WITH `if_exists: true` (assert clean no-op /
   `existed: false`).

## Required work

Pick the right home: (1) and (2) fit naturally in
`tests/e2e/tests/14-index2-types.test.js` (JS suite, already has sorted/
covering-adjacent index2 tests) OR a more general DDL test file — check
existing file sizes/conventions first, your call. (3) likely fits better in
an existing DDL-focused file (check both suites for where `dropIndex`/
`drop_index` already has SOME test, even if not `if_exists`, and extend
there) — again your call, but stay consistent with existing conventions
rather than creating a new file for 3 small additions.

Use ONLY query builders (`ddl.createIndex(...)`, `ddl.dropIndex(...)`,
`filter.and(...)`, etc.) — no hand-assembled wire objects (repo-wide
CLAUDE.md rule).

## Verification

- `cd tests/e2e && node e2e.test.js` — baseline after #974/#975/#976 is
  18 files / 137 passed / 0 failed. Report exact counts before and after.
- If you touch `crates/shamir-client-ts`, run its vitest suite too and
  report pass/fail counts.

## Scope discipline

- Do NOT attempt to implement composite sorted indexes or fix the
  "composite TBD" limitation — that's a real product gap, not this task;
  just prove the current reject behavior is correct and observable.
- Do NOT touch DDL singletons (rename_db/describe_table/change_password/
  interactive tx — cluster E, #978) or keyset pagination (cluster F, #979).
- Do NOT modify production Rust code. If you find a real bug (e.g. the
  `include`-on-non-sorted rejection doesn't actually happen, or composite
  regular-index matching returns wrong rows), STOP and report it instead of
  silently working around or "fixing" it.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit test files and run read-only/test
commands.

## What to report back

List every test added and what it proves (especially the exact
`stats.index_used` string each positive test asserts, and the exact error
each negative test asserts). Confirm ONLY query builders were used. Give
exact test-run output with real pass/fail counts.
