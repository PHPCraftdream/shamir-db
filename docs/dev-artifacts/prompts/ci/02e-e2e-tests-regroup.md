# CI-02e — regroup `src/__tests__/` (34 entries) and split two oversized files

Parent brief: `docs/dev-artifacts/prompts/ci/02-layout-sweep-client-ts.md`.
Pure structural refactor: every test keeps its EXACT full name (describe
titles + it titles byte-identical); vitest's default include picks up
`*.test.ts` anywhere. Files are moved byte-identical EXCEPT relative import
depths (from the new subfolders: `'../index.js'` → `'../../index.js'`,
`'./e2e-harness.js'` → `'../e2e-harness.js'`, `'../core/…'` → `'../../core/…'`).

## Rules (binding)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `git rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits. Moving = create new + `rm` old.
- NO `cargo` / `npm` / `tsc` / `vitest` / `node` or any compile/test/install
  command. Allowed: read-only git, `rg`, `wc -l`,
  `python -I scripts/ci/check_layout.py`, file tools, `mkdir`, `rm`.
- Imports at the top; preserve comments; LF endings; no `@ts-ignore`.
- Test names and skip-gating behavior must be preserved EXACTLY.

## Part 1 — folder moves (create under src/__tests__/)
- `connection/`: client-demux, connect, resume, e2e-connect-local,
  e2e-resume, e2e-subscriptions (6 test files)
- `auth/`: e2e-permissions, e2e-principal, e2e-change-password (3)
- `data/`: e2e-data (see Part 2 split), e2e-cursors, e2e-cursor-lifecycle,
  e2e-keyset, e2e-u64-big, e2e-interner (7 files after split)
- `query/`: e2e-cond, e2e-explain-having, e2e-nulls-order, e2e-when,
  e2e-fts, e2e-vector, e2e-for-each (7)
- `ddl/`: e2e-ddl, e2e-rename-table, e2e-rename-index, e2e-rename-repo,
  e2e-schema-validators (5)
- `engine/`: e2e (see Part 2 split), e2e-call, e2e-batch-limits,
  e2e-batch-sequencing, e2e-fk-ondelete, e2e-version-cas (7 after split)
- `e2e-harness.ts` STAYS at src/__tests__/ root. Final root = 7 entries
  (harness + 6 subfolders); every subfolder ≤7.

## Part 2 — split the two oversized files (each part ≤1000 lines)
- `e2e-data.test.ts` (1285): single top-level describe with topic it-groups.
  Split before the `pattern-setup: create db + seed` it (~line 699).
  Part 1 keeps the name `e2e-data.test.ts` (through the interner group);
  part 2 = new `src/__tests__/data/e2e-data-query.test.ts` (pattern/ilike/
  regex, isNull/exists filters, contains*, pagination/distinct, select.func/
  aggregateFn, history, fieldEq/$expr/$fn, distinct aggregates). BOTH parts
  use the IDENTICAL top-level describe title as the original and replicate
  the original's preamble: imports, harness/skip-gating mechanism (read
  e2e-harness.ts + the file header to copy it verbatim), shared beforeAll/
  beforeEach setup the moved its depend on, and any file-local helpers those
  its use. The trailing `describe('e2e-data.test skip reason', …)` block
  stays ONLY in part 1, byte-identical.
- `e2e.test.ts` (1049): split before the `DDL: list databases includes
  default` it (~line 537). Part 1 keeps `e2e.test.ts` (connect/CRUD/filters/
  agg/sort-page/batch sections); part 2 = new
  `src/__tests__/engine/e2e-tx-ddl.test.ts` (DDL/HMAC/tx/itx/scram/handle/
  db.tx/nested sections) with identical describe title, preamble, and
  skip-gating; its `describe('e2e.test skip reason', …)` stays ONLY in
  part 1.
Duplicated preamble/helpers between the two parts of a split are acceptable;
do NOT invent new abstractions. If a shared helper is large, a small
non-test module next to the tests in the same subfolder is allowed.

## Verify
- `python -I scripts/ci/check_layout.py` exits 0.
- Count check: 33 original test files → 33 + 2 new part files = 35
  `*.test.ts` under src/__tests__ (rg -c). Every it/describe title from the
  two original files appears exactly once across the split parts (compare
  against `git show HEAD:crates/shamir-client-ts/src/__tests__/<file>`).
- No remaining `'./e2e-harness.js'` importers below src/__tests__ root, and
  every subfolder test uses `'../../index.js'` / `'../e2e-harness.js'` depths.
Report: final tree with per-folder counts, the two split seams, preamble
handling notes.
