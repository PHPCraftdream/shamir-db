# Task #920 -- rewrite the 17 tests/e2e test files to use @shamir/client query builders (Stage B)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

This is Stage B of the tests/e2e query-builder rewrite (Stage A, task
#919, already landed as commit `aef92b54` -- read it via `git show
aef92b54` for full context and the exact pattern to follow). Stage A
rewrote `tests/e2e/helpers/fixtures.js` and `tests/e2e/helpers/hmac.js` to
build wire ops via `@shamir/client`'s platform-agnostic query-builder
library (`ddl.*`/`admin.*`/`write.*`, imported with plain synchronous
`require('@shamir/client')` -- Node 22.12+ supports `require()` of an ESM
package with no top-level await; CI pins `node-version: '22'`, well past
22.12, and this was verified working both locally and is expected to work
identically on CI).

`@shamir/client` is already a devDependency of `tests/e2e` (added in
Stage A). The `signerFor(client)` adapter already exists in
`tests/e2e/helpers/hmac.js` (exported) -- reuse it, do not reinvent it:
```js
const { signerFor } = require('../helpers/hmac');
// or within a test file: const hmac = require('../helpers/hmac'); hmac.signerFor(client)
```

## What's left (raw wire-object construction NOT covered by Stage A)

Per a `grep -rohE` sweep over `tests/e2e/tests/*.js` (2026-08-02 count):
`create_index` (25x), `chmod` (9x), `create_table` (6x -- some inline,
outside `fixtures.setupDb`), `drop_table` (4x -- some inline, not via
`hmac.drop_table_op`), `create_repo` (4x), `create_db` (4x),
`start_migration` (3x -- some inline, not via `hmac.start_migration_op`),
`drop_db` (1x). Concentrated in:
`08-admin-ddl.test.js`, `12-hmac-gate.test.js` (uses `hmac.js` helpers
already, mostly fine -- spot-check), `13-migration.test.js`,
`14-index2-types.test.js`, `16-replication.test.js`,
`17-replication-convergence.test.js`, `18-vectors.test.js`. Read each
file before editing -- don't assume the grep counts map 1:1 to files
needing changes; some `create_index`/`create_table` calls may already go
through `fixtures.setupDb` (fine, no change needed) vs. being constructed
inline in the test body (needs the builder).

**The concrete, currently-failing bug this fixes (task #917 / "CI-2"):**
`16-replication.test.js` lines 101-112 build three raw
`{ chmod: {...}, mode: MODE_777 }` queries with NO `hmac` field --
confirmed via a real CI run (`30744506949`, then reproduced again in
`30745651338`) that this throws `db error [hmac_required]` in the
`setup` test, cascading into every other test in that file AND in
`17-replication-convergence.test.js` (same pattern, same file-setup
shape). Fix: use `admin.chmod(hmac.signerFor(client), <ResourceRef>,
mode)`. `ResourceRef` constructors are in `admin.ts`:
`admin.refDatabase(db)`, `admin.refStore(db, repo)`,
`admin.refTable(db, repo, table)` -- e.g.:
```js
const { admin } = require('@shamir/client');
const hmac = require('../helpers/hmac');
// ...
await client.execute(db, {
  id: 'repl-chmod-db',
  queries: { c: admin.chmod(hmac.signerFor(client), admin.refDatabase(db), MODE_777) },
});
```
Do this file FIRST and land it as its OWN commit (it directly resolves
#917/CI-2, currently failing on every CI run) -- then continue the rest
of the sweep.

## Per-op builder mapping (check `ddl.ts`/`admin.ts` for exact signatures
before using -- these are pointers, not copy-paste-ready snippets)

- `create_index` → `ddl.createIndex(name, table, fields, opts)` --
  `opts.unique`/`opts.sorted`/`opts.repo`/`opts.vector_dim`/
  `opts.vector_metric`/`opts.vector_quantization`/`opts.fts_tokenizer`/
  etc. Read `14-index2-types.test.js` and `18-vectors.test.js`'s current
  inline shapes carefully -- vector/FTS index options have many optional
  fields, get the mapping exactly right (a silently-wrong option is worse
  than a compile error).
- `create_table`/`create_repo`/`create_db` inline (not via
  `fixtures.setupDb`) → `ddl.createTable(name, opts)` /
  `ddl.createRepo(name, opts)` / `ddl.createDb(name, opts)`.
- `chmod`/`chown`/`chgrp` → `admin.chmod(signer, resource, mode)` /
  `admin.chown(signer, resource, owner)` / `admin.chgrp(signer, resource,
  group)`.
- `drop_table`/`drop_db`/`drop_index` inline (not via the existing
  `hmac.drop_*_op` helpers) → prefer calling the EXISTING
  `hmac.drop_table_op(client, ...)` etc. helpers from Stage A instead of
  reaching for `ddl.dropTable` directly -- they already wrap the builder +
  signer. Only reach for `ddl.*` directly if a helper doesn't exist for
  that exact op.
- `start_migration` inline → prefer `hmac.start_migration_op(client, ...)`
  (already exists) over calling `ddl.startMigration` directly.
- `create_scram_user`/`createScramUser` calls are FINE as-is -- that's a
  dedicated native napi method (`client.createScramUser(name, pw, roles)`)
  that computes its own hmac tag internally in Rust
  (`crates/shamir-client/src/client.rs::create_scram_user`), not a
  `queries: {...}` batch entry. Do NOT touch these.

## What NOT to do

- Do NOT change any test assertion or expected behavior -- same queries,
  same expected results, purely a construction-mechanism change.
- Do NOT touch `helpers/fixtures.js` or `helpers/hmac.js` further unless
  you find an actual bug in Stage A's rewrite while working here (if so,
  fix it and flag it clearly in your report -- don't silently patch over
  it).
- If `@shamir/client`'s builders don't cover some exact op/option
  combination you find, leave that ONE call site hand-rolled with a
  one-line comment stating why (mirroring `drop_role_op`'s precedent in
  `helpers/hmac.js`) -- don't block the whole sweep on one gap.

## Verification

The release server binary and napi binding are ALREADY built and current
(no Rust source changed by this task) -- do NOT waste time rebuilding
unless you've somehow determined they're stale. Run `cd tests/e2e && node
e2e.test.js` after your changes and compare against this EXACT known-good
baseline (confirmed 2026-08-02, post-Stage-A):
```
files:  18
passed: 121
failed: 9
```
The 9 pre-Stage-B failures are: 2 in `13-migration.test.js` ("full
migration lifecycle", "migration rollback cleans up" -- both fail with
`experimental_feature_disabled`, an UNRELATED pre-existing/intentional gate,
not yours to fix), and 7 across `16-replication.test.js` +
`17-replication-convergence.test.js` (the chmod hmac_required cascade this
task fixes). After your fix, expect:
```
files:  18
passed: 128 (121 + 7 newly-fixed)
failed: 2   (the two unrelated experimental-migration-api failures only)
```
If you see a DIFFERENT failure count/set, you introduced a regression --
find and fix it before considering this done, don't just report the
discrepancy.

## Definition of done

- Every raw hand-assembled destructive-op / create-op / index-op wire
  object in the 17 test files that has a builder equivalent now goes
  through it (directly or via the `helpers/hmac.js` wrappers).
- `16-replication.test.js` and `17-replication-convergence.test.js` no
  longer fail with `hmac_required` -- their full ReplHello/ReplPull/
  deny-by-default scenarios pass.
- Local `node e2e.test.js` run matches the expected 128 passed / 2 failed
  baseline above.
- Commit per logical group (e.g. the replication files as one commit
  since they share the exact bug, then index/vector files, then the rest)
  -- not one giant commit, per this repo's commit-hygiene convention.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
