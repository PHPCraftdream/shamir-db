# Brief — e2e gap cluster C: functional-index & FTS option breadth

Task: #976 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, "Cluster C — Functional-index & FTS option breadth".

## ⚠️ Correction to the source matrix — read before starting

The matrix claims 6 functional ops (`lower/upper/trim/length/substring/mod`)
plus `coalesce`/`concat` are valid `functional_op`/`Computed.expr_op`
values. **This is wrong** — verified against current source:

- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` (~line 152-176,
  functional index DDL creation) and `crates/shamir-engine/src/query/filter/compile.rs`
  (`build_index_expr`, ~line 190-214, the matching query-side compiler) both
  only recognize `"lower"`, `"upper"`, `"trim"`, `"length"`, `"field"` as
  built-in `functional_op`/`expr_op` values. Any other string is treated as
  a **funclib-registered scalar name** (`IndexExpr::Scalar`) — it is NOT a
  built-in op.
- `"mod"`, `"substring"`, `"coalesce"`, `"concat"` are a **completely
  different, unrelated feature**: `FilterExprOp` in
  `crates/shamir-engine/src/query/common/parser.rs` (~line 400-420), used
  for `$expr`/`$fn` **value expressions** in writes/batches (see
  `crates/shamir-engine/src/query/batch/README.md` "concat"/"mod" rows).
  They have nothing to do with functional indexes or `Computed` filters.
  That gap is already tracked separately under cluster G (`$expr`/`$fn` in
  values, task #980) — **do NOT touch it here**, it would be duplicate/wrong
  scope.

**Actual scope of this task is narrower than the matrix implies:**

1. **`trim` and `length` functional-index ops** — only `lower`/`upper` are
   exercised live today (`tests/e2e/tests/14-index2-types.test.js`,
   "functional: LOWER(email) = lookup" / "functional: UPPER lookup"). Add
   the same shape of test for `trim` and `length`: create a `functional`
   index with `functional_op: 'trim'` (resp. `'length'`), then query with
   `filter.computed('trim', field, 'eq', value)` (resp. `'length'` with an
   integer `value`). Follow the existing two tests' exact pattern (same
   file, same fixture helpers).
2. **FTS `unicode` tokenizer** — only the default `whitespace` tokenizer is
   exercised. Add a test creating an FTS index with `fts_tokenizer: 'unicode'`
   and prove it actually tokenizes (e.g. a query that only succeeds if
   unicode-aware boundary splitting happened — pick a case that would behave
   differently than `whitespace` if you can construct one simply; otherwise
   at minimum prove the option round-trips and FTS matching still works).
3. **`fts_language`** — read `crates/shamir-query-types/src/admin/types/index_ops.rs`
   (~line 40-46): the field's own doc comment says it's "for future
   stemming" and is currently NOT consumed for any tokenization behavior
   change (confirmed: no live engine code path reads `fts_language` for
   anything except storing it — grep `fts_language` under
   `crates/shamir-engine/src` yourself to confirm before writing the test).
   So the only honest assertion here is: `create_index` with
   `fts_language: 'en'` (or similar) is **accepted without error**, and FTS
   querying against that index still works normally. Do NOT assert any
   stemming/language-specific matching behavior — it doesn't exist yet.

## Required work

Extend `tests/e2e/tests/14-index2-types.test.js` (JS e2e suite,
`node tests/e2e/e2e.test.js`) using ONLY the existing `ddl.createIndex(...)`
and `filter.computed(...)`/`filter.fts(...)` query builders already used in
that file — no hand-assembled wire objects (repo-wide CLAUDE.md rule).

Add:
- `functional: TRIM(field) = lookup` test (mirror the LOWER/UPPER tests).
- `functional: LENGTH(field) = lookup` test (mirror the LOWER/UPPER tests,
  `value` is an integer).
- `fts: unicode tokenizer accepted and matches` test.
- `fts: language hint accepted (no-op today)` test.

## Verification

- `cd tests/e2e && node e2e.test.js` — baseline after #974/#975 is
  18 files / 133 passed / 0 failed. Report exact counts before and after.
- If you touch anything in `crates/shamir-client-ts`, also run its vitest
  suite and report pass/fail counts — but this task's scope is the JS
  suite only, so that should not be necessary.

## Scope discipline

- Do NOT touch `$expr`/`$fn` value-expression ops (`mod`/`substring`/
  `coalesce`/`concat`) — that's cluster G (#980), a different feature
  entirely (see correction above).
- Do NOT touch covering `include`/composite indexes (cluster D, #977) or
  DDL singletons (cluster E, #978).
- Do NOT modify production Rust code. If you find a real bug while writing
  these tests, STOP and report it instead of silently working around or
  "fixing" it.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit test files and run read-only/test
commands.

## What to report back

List every test added and what it proves. Confirm you did NOT touch
`mod`/`substring`/`coalesce`/`concat` (out of scope per the correction
above). Give exact `node e2e.test.js` output with real pass/fail counts.
