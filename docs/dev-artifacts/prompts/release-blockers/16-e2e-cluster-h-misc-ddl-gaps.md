# Brief — e2e gap cluster H (low priority): misc DDL gaps

Task: #981 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, "Cluster H — Misc low-priority DDL gaps". Last cluster in the e2e-gap sweep.

## ⚠️ Scope correction — `fts_language` is ALREADY covered, skip it

The matrix's cluster H list includes `fts_language` hint, but this was
ALREADY closed in task #976 (`fts: language hint accepted (no-op today)`
test in `tests/e2e/tests/14-index2-types.test.js`) — the matrix's cluster
C and cluster H both independently listed it, a genuine duplicate in the
source document. **Do NOT add another `fts_language` test** — verify #976's
test exists (`git log --oneline -- tests/e2e/tests/14-index2-types.test.js`
or just read the file) and move on. This brief covers the remaining 5 items.

## The 5 remaining gaps

All source types: `crates/shamir-query-types/src/admin/types/validator_ops.rs`,
`function_ops.rs`, `schema_ops.rs`, `interner_ops.rs`. All builders already
exist in `crates/shamir-client-ts/src/core/builders/ddl.ts` — confirmed:
`renameValidator`, `renameFunctionFolder`, `getTableSchema`, `addSchemaRule`,
`removeSchemaRule`, `internerDump`. Read the exact signatures there before
writing code — do not guess parameter shapes.

1. **`rename_validator`** live execution. `RenameValidatorOp`
   (`validator_ops.rs` ~line 43-52): `{ rename_validator: old, to: new }`.
   Existing precedent: `crates/shamir-client-ts/src/__tests__/e2e-schema-validators.test.ts`
   already has `createValidator`/`bindValidator`/`dropValidator` coverage —
   extend that SAME file/setup rather than duplicating fixtures. Test: create
   a validator, rename it, assert the validator is still bound/functional
   under the NEW name (e.g. `list_validators` shows the new name, or a
   write that should be rejected by the validator still is, post-rename).

2. **`rename_function_folder`** live execution. `RenameFunctionFolderOp`
   (`function_ops.rs` ~line 102-115): both `rename_function_folder` and `to`
   are path-segment vectors (`Vec<String>`); renames the folder AND every
   descendant whose path is prefixed by it. Existing precedent:
   `crates/shamir-client-ts/src/__tests__/e2e-ddl.test.ts` ~line 239 already
   has a `createFunctionFolder` + `createFunction` + `listFunctions` +
   `renameFunction` + `dropFunction` test — extend that SAME test/setup
   (don't rebuild a folder+function fixture from scratch). Test: create a
   folder with at least one function inside it, rename the FOLDER (not the
   function), assert the function is now reachable/listed under the new
   folder path and the OLD folder path is gone.

3. **Incremental schema-mutation ops**: `get_table_schema`, `add_schema_rule`,
   `remove_schema_rule` (`schema_ops.rs` ~line 236-277). Today only
   `set_table_schema` (whole-replace) is exercised. Test: create a table with
   NO schema, `add_schema_rule` one rule, `get_table_schema` and assert it
   appears; `add_schema_rule` a SECOND rule (different path), `get_table_schema`
   again and assert BOTH appear; `add_schema_rule` again on the FIRST rule's
   path with different constraints (upsert-by-path semantics per the doc
   comment) and assert it REPLACED (not duplicated) the first rule;
   `remove_schema_rule` the second rule's path, `get_table_schema` and assert
   only the first (replaced) rule remains.

4. **`interner_dump`** wire op (`interner_ops.rs` ~line 14-33): `{
   interner_dump: repo }` returns the full name→id dictionary + current
   epoch; `{ interner_dump: repo, since: N }` returns only entries with
   `id > N` (delta refresh). Test: insert/create a few fields (via any
   write/schema op that touches new field names) to populate the interner,
   `internerDump(repo)` with no `since` and assert the known field names
   appear with sane ids; capture the current epoch, add ONE more new field
   name, `internerDump(repo, { since: capturedEpoch })` and assert ONLY the
   new entry comes back (delta semantics proven, not just "some data").

5. **`expected_version` CAS on `set_table_schema`** (`schema_ops.rs` ~line
   229-234, `SetTableSchemaOp.expected_version`). Server-side enforcement:
   `crates/shamir-db/src/shamir_db/execute/admin_schema.rs` ~line 457-460,
   error code `version_conflict` on mismatch. Test: `set_table_schema` once
   (get its resulting `schema_version` from the response or a subsequent
   `get_table_schema`), then `set_table_schema` AGAIN with the CORRECT
   `expected_version` (must succeed), then a THIRD time with a STALE/wrong
   `expected_version` (must fail with an error containing `version_conflict`).

## Required work

Extend the existing files named above per item — do NOT create new files for
these 5 items unless an item genuinely has no natural home (unlikely; all 5
extend fixtures that already exist). Use ONLY query/DDL builders — no
hand-assembled wire objects (repo-wide CLAUDE.md rule).

## Verification

- Run the full vitest suite in `crates/shamir-client-ts` (`npx vitest run`)
  — baseline after #980 is 57 files / 1050 tests passed. Report exact
  counts before and after.
- `npx tsc --noEmit` in that package — must stay clean.
- If you touch the JS suite, also run `cd tests/e2e && node e2e.test.js`
  (baseline: 19 files / 147 passed) and report counts.

## Scope discipline

- Do NOT add another `fts_language` test — see correction above.
- Do NOT modify production Rust or the DDL builders. If any of the 5 items
  behaves differently than documented (e.g. `add_schema_rule` doesn't
  actually upsert-by-path, or `interner_dump`'s `since` filter doesn't
  correctly exclude old entries), STOP and report it as a real bug/doc
  mismatch — do not silently adjust the test to match broken behavior
  (see task #983, filed this session, for the pattern to follow: a genuine
  bug found during this exact kind of verification work gets its own
  tracked task, not a quietly-weakened assertion).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create test files and run read-only/test
commands.

## What to report back

List every test added and what it proves. Confirm you did NOT duplicate the
`fts_language` test. Give exact test-run output with real pass/fail counts.
This is the LAST e2e-gap cluster — after this, the whole #964 coverage
sweep (clusters A-H) is complete.
