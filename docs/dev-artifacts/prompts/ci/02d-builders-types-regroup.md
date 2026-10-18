# CI-02d — regroup `core/builders/` (split ddl.ts) and `core/types/`

Parent brief: `docs/dev-artifacts/prompts/ci/02-layout-sweep-client-ts.md`.
builders/ = 14 entries (ddl.ts 1407 lines), builders/__tests__/ = 15,
core/types/ = 12. Pure structural refactor; every previously exported NAME
stays exported with identical spelling from the same barrels
(core/builders/index.ts, core/types/index.ts, src/index.ts, src/browser.ts —
the last two must need NO edits).

## Rules (binding)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `git rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits. Moving = create new + `rm` old.
- NO `cargo` / `npm` / `tsc` / `vitest` / `node` or any compile/test/install
  command. Allowed: read-only git, `rg`, `wc -l`,
  `python -I scripts/ci/check_layout.py`, file tools, `mkdir`, `rm`.
- Imports at the top; index.ts files re-export only (the ddl namespace object
  in ddl/index.ts is the one allowed composed constant). Preserve comments;
  no reformatting; LF endings; no `@ts-ignore`.

## Part 1 — split builders/ddl.ts (1407 lines) into builders/ddl/
Read the file in full. Split along the export seams; each part keeps the
moved functions' bodies, doc comments and export keywords byte-identical:
- `ddl/ddl-index.ts`: `createIndex`, types `Tokenizer`/`Metric`/`Quantization`,
  `hashIndex`, `uniqueIndex`, `sortedIndex`, `sortedWithIncludeIndex`,
  `ftsIndex`, `functionalIndex`, `vectorIndex`, `dropIndex`, `renameIndex`.
- `ddl/ddl-schema.ts`: `DEFAULT_REPO`, `repoOrDefault` (export it — siblings
  use it), `createDb`, `createTable`, `dropDb`, `dropRepo`, `dropTable`,
  `renameTable`, `renameRepo`, `renameDb`, `setTableSchema`, `addSchemaRule`,
  `removeSchemaRule`, `getTableSchema`, `describeTable`, `startMigration`,
  `commitMigration`, `rollbackMigration`.
- `ddl/ddl-func.ts`: `createFunction`, `dropFunction`, `renameFunction`,
  `createValidator`, `dropValidator`, `renameValidator`, `bindValidator`,
  `unbindValidator`, `listValidators`, `createFunctionFolder`,
  `renameFunctionFolder`, `listFunctions`, `listValidators_`,
  `listFunctionFolders`.
- `ddl/ddl-misc.ts`: `currentOnly`, `olderThan`, `olderThanAge`,
  `setBufferConfig`, `getBufferConfig`, `alterBufferConfig`,
  `migrationStatus`, `setRetention`, `purgeHistory`, `changesSince`,
  `internerDump`, `internerTouch`, `listDatabases`, `listRepos`,
  `listTables`, `listIndexes`, `listUsers`.
- `ddl/field-builder.ts`: `FieldBuilder` class + `field()`.
- `ddl/index.ts`: re-export barrel of ALL names ddl.ts exported (exact name
  set — compare against `git show HEAD:crates/shamir-client-ts/src/core/builders/ddl.ts`
  export list), the passthrough
  `export { createRepo, alterRepo, describeRepo } from '../admin/repo-admin/index.js';`,
  and `export const ddl = { … }` composed EXACTLY as the original (same
  properties, same order), importing the parts. Every file ≤1000 lines.
Then `rm builders/ddl.ts`.

## Part 2 — builders folder regroup
- `select.ts`, `cursor.ts`, `query.ts`, `filter.ts` → `builders/read/`
- `write.ts`, `batch.ts`, `call.ts` → `builders/write/`
- `replication.ts`, `subscribe.ts` → `builders/stream/`
- `handle.ts` STAYS at builders root. `admin/` stays.
- builders/index.ts: rewrite specifiers only ('./filter.js' → './read/filter.js',
  './write.js' → './write/write.js', './ddl.js' → './ddl/index.js',
  './replication.js' → './stream/replication.js', './subscribe.js' →
  './stream/subscribe.js', './query.js' → './read/query.js', './call.js' →
  './write/call.js', './batch.js' → './write/batch.js', './cursor.js' →
  './read/cursor.js'; handle/admin unchanged). All names identical.
Final builders/ root (7): admin/, ddl/, read/, write/, stream/, handle.ts,
index.ts.

## Part 3 — builders/__tests__ distribution (fix import depths per file)
- `builders/ddl/__tests__/`: existing `ddl/` test subfolder (ddl.test.ts,
  ddl-schema.test.ts, durability.test.ts) + `create_index_matrix.test.ts` +
  `create_index_typed.test.ts`
- `builders/read/__tests__/`: filter, query, select, cursor,
  vector_filter_parity tests
- `builders/write/__tests__/`: write, batch, call tests
- `builders/stream/__tests__/`: replication, subscribe, repl_parity tests
- `builders/admin/__tests__/`: admin.test.ts
Each moved test: byte-identical except relative import fixes (they import
builder modules/types directly — re-depth to the new homes).

## Part 4 — core/types/ regroup
- `types/read/`: filter.ts, query.ts, cursor.ts
- `types/write/`: write.ts, batch.ts, call.ts (+ move `types/__tests__/` or
  `core/types/__tests__/batch-types.test.ts` from stage 02c to
  `types/write/__tests__/` if it tests types/batch.ts — check its imports;
  fix depths)
- `types/schema/`: ddl.ts;  `types/access/`: admin.ts
- `types/stream/`: replication.ts, subscribe.ts
- `connection.ts` STAYS at types root; index.ts stays, specifiers rewritten
  ('./filter.js' → './read/filter.js', etc.). All exported names identical.
- Fix intra-types imports (admin.ts imports write/filter/ddl; batch.ts imports
  many siblings; cursor.ts imports query+batch; query.ts imports filter) and
  ALL outside importers of the moved types files: builders parts (e.g.
  `'../../types/batch.js'` → `'../../types/write/batch.js'`), handle.ts,
  core/client/*, core/support/exec-ctx.ts, receipts/*, tests — grep each moved
  basename (`types/filter.js`, `types/query.js`, `types/cursor.ts`,
  `types/write.js`, `types/batch.js`, `types/call.js`, `types/ddl.js`,
  `types/admin.js`, `types/replication.js`, `types/subscribe.js`) across
  crates/shamir-client-ts/src and re-point. `types/index.js` and
  `types/connection.js` specifiers are UNCHANGED everywhere.

Final types/ root (7): index.ts, connection.ts, read/, write/, schema/,
access/, stream/.

## Verify
- `python -I scripts/ci/check_layout.py` exits 0.
- Name-set equality: builders/index.ts + types/index.ts export lists vs
  `git show HEAD:...` versions (same names; only specifier paths differ).
- No stale old specifiers anywhere under src/ (rg per moved basename).
- Every moved file exists exactly once; no file >1000 lines (`wc -l`).
Report: trees with entry counts, ddl split line counts, files whose imports
were rewritten.
