# Task #919 -- wire up @shamir/client query builders in tests/e2e (Stage A: shared helpers only)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`tests/e2e` (the node napi e2e harness under `tests/e2e/`, plain CommonJS
`.js` files, using the raw `shamir-client` native binding from
`crates/shamir-client-node` -- NOT the TS/WS client) hand-assembles wire-shape
query objects directly (`queries: { key: { create_db: name } }` etc.) instead
of going through a query builder. This violates this repo's own convention
(`CLAUDE.md` "Query construction -- builder only"): every wire op should be
built via a builder, never hand-assembled raw JSON/objects, except at
documented boundaries (none of which apply here -- this is neither an
FFI-deserialization site, a serde round-trip test, nor a WASM-bridge
conversion).

This was discovered while investigating a REAL, currently-failing CI bug
(task #917 / "CI-2"): `tests/e2e/tests/16-replication.test.js`'s setup test
builds three raw `{ chmod: {...}, mode }` batch queries by hand with NO
`hmac` field. The server's `check_destructive_hmacs`
(`crates/shamir-server/src/db_handler/admin.rs:637`) requires an `hmac` tag
on `Chmod` (and `Chown`/`Chgrp`/`CreateUser`/`Drop*`/`GrantRole`/etc.) --
so the setup step fails immediately with `db error [hmac_required]`, and
every downstream test in that file (and in
`17-replication-convergence.test.js`, which has the same pattern) cascades
into unrelated-looking failures. Confirmed via a real CI run
(`30744506949`, dispatched 2026-08-02) -- 9 failing tests, all traced back
to this ONE root cause.

**This task (Stage A) does NOT fix that specific test file yet** -- that's
task #920's job, once this task lands the underlying builder plumbing.
This task's job is the shared infrastructure: `helpers/fixtures.js` and
`helpers/hmac.js`, which almost every one of the 18 test files depends on.

## What's already been investigated -- don't re-investigate this

- `crates/shamir-client-ts` (npm package `@shamir/client`) already has a
  full, explicitly **"PLATFORM-AGNOSTIC"** query-builder library at
  `crates/shamir-client-ts/src/core/builders/{ddl,admin,write,filter,...}.ts`.
  Read `ddl.ts` and `admin.ts` in full before starting -- they cover
  `createDb`/`createRepo`/`createTable`/`createIndex`/`dropDb`/`dropRepo`/
  `dropTable`/`dropIndex`/`dropUser`/`startMigration`/`commitMigration`/
  `rollbackMigration`/`setRetention`/`purgeHistory` (in `ddl.ts`) and
  `chmod`/`chown`/`chgrp`/`createGroup`/`dropGroup`/`renameGroup`/
  `addGroupMember`/`removeGroupMember`/`createUser`/`dropUser`/
  `setSuperuser`/`grantRole`/`revokeRole` (in `admin.ts`).
- These builders are publicly re-exported from the package root:
  `crates/shamir-client-ts/src/index.ts` line 49,
  `export * from './core/builders/index.js';` -- so
  `import { ddl, admin } from '@shamir/client'` (or named imports of
  individual functions) works once the package is a dependency.
- `crates/shamir-client-ts/dist/` is already built (ESM output,
  `crates/shamir-client-ts/package.json` has `"type": "module"`). If it
  looks stale, run `npm run build` in `crates/shamir-client-ts` first (do
  NOT edit dist/ by hand).
- `tests/e2e` is CommonJS (`'use strict'`, `require(...)`, no `"type":
  "module"` in `tests/e2e/package.json`). Node's CommonJS CAN
  `await import('@shamir/client')` (dynamic `import()` works from CJS to
  load an ESM package) -- every test file's exported function is already
  `async function ({ client, server, fixtures, test, assert, assertEq })`,
  and `helpers/fixtures.js`/`helpers/hmac.js`'s functions are already
  `async` too, so adding a top-of-function (or module-level, cached)
  `const { ddl, admin } = await import('@shamir/client');` is a non-issue.
  Confirm this actually works in practice as your FIRST step (a tiny throwaway
  script) before wiring it through everywhere -- don't assume and discover a
  packaging mismatch 40 minutes in.
- `HmacSigner` (the interface the builders' HMAC-gated functions expect) is
  trivial: `{ hmacTagHex(canonical: Uint8Array): string }`
  (`crates/shamir-client-ts/src/core/types/ddl.ts:183`).
  `tests/e2e/helpers/hmac.js` ALREADY derives the same key-derivation +
  signing logic (`deriveKey(client.sessionId())` then
  `crypto.createHmac('sha256', key).update(canonical).digest('hex')`) --
  this MUST match `shamir_connect::common::crypto::derive_session_hmac_key`
  on the Rust side byte-for-byte (it's the same domain-separated SHA-256
  scheme used by `crates/shamir-client/src/client.rs`'s
  `create_scram_user`). Build a tiny adapter object around this existing
  logic:
  ```js
  function signerFor(client) {
    return { hmacTagHex: (canonical) => sign(client, Buffer.from(canonical)) };
  }
  ```
  Do NOT reimplement the signing logic a second time -- reuse `sign()`
  from the existing `helpers/hmac.js` (or move it, see below).

## What to do

1. Add `@shamir/client` as a dependency in `tests/e2e/package.json`,
   mirroring the existing `"shamir-client": "file:../../crates/shamir-client-node"`
   pattern: `"@shamir/client": "file:../../crates/shamir-client-ts"`. Run
   `npm install` in `tests/e2e/` afterward. Confirm
   `crates/shamir-client-ts/dist` is built and current (rebuild if needed --
   `cd crates/shamir-client-ts && npm run build`).
2. Rewrite `tests/e2e/helpers/fixtures.js`'s `setupDb` and `seed` functions
   to build their `create_db`/`create_repo`/`create_table` ops via
   `ddl.createDb(...)`/`ddl.createRepo(...)`/`ddl.createTable(...)` instead
   of hand-rolled `{ create_db: name }` objects. `seed`'s `set` ops: check
   `core/builders/write.ts` first for whatever builder covers a plain
   keyed upsert (`set`) -- if none exists for this exact non-HMAC-gated
   write shape, leave `seed`'s raw `{ set: table, key, value }` construction
   as-is and note in your final report why (non-HMAC-gated plain writes may
   not need a builder if the wire shape is already this simple and there's
   no canonical/HMAC computation involved -- the builder-only rule exists to
   prevent HAND-COMPUTING signed/canonical bytes incorrectly, not to force
   every trivial object literal through a wrapper function).
3. Rewrite `tests/e2e/helpers/hmac.js`'s `drop_db_op`/`drop_repo_op`/
   `drop_table_op`/`drop_index_op`/`drop_user_op`/`drop_role_op`/
   `start_migration_op`/`commit_migration_op`/`rollback_migration_op` to
   delegate to `ddl.dropDb(signerFor(client), ...)` etc. instead of manually
   building the canonical byte string + calling `sign()` inline for each op
   type -- the duplication between this file's canonical-byte construction
   and `@shamir/client`'s internal `canonicalDropDb`/etc. is itself a
   correctness risk (the two could silently drift out of sync). Keep the
   exported function names and call signatures IDENTICAL (`drop_table_op(client,
   dbInUse, repo, table)` etc.) so the 17 consuming test files (task #920's
   scope, NOT this task) don't need to change yet.
4. Do NOT touch any of the 17 files under `tests/e2e/tests/` in this task
   (that's #920) -- this task is additive/internal-refactor only to the two
   shared helper files plus `package.json`.

## Verification (mandatory before considering this done)

- `cd tests/e2e && npm run build:server && npm run build:binding` (rebuilds
  the release server + napi binding from current source -- required once
  before `npm test` will reflect current server behavior).
- `cd tests/e2e && npm test` -- run the FULL existing suite locally and
  confirm IDENTICAL pass/fail counts to a run from before your changes
  (i.e., this refactor changes HOW queries are built, not WHAT queries are
  sent or WHAT the expected results are -- if a test that passed before now
  fails, that's a bug in your rewrite, not a pre-existing issue to paper
  over).
- If `16-replication.test.js`/`17-replication-convergence.test.js` still
  fail with `hmac_required` after this task -- that's EXPECTED (their inline
  `chmod` calls aren't touched until #920) -- do not attempt to fix that in
  this task.

## Definition of done

- `helpers/fixtures.js` and `helpers/hmac.js` contain zero hand-assembled
  destructive-op wire objects or manually-computed HMAC canonical
  bytes/signing calls for anything `@shamir/client`'s builders already
  cover -- everything routes through the builder + a shared `signerFor()`
  adapter.
- All 18 existing `tests/e2e/tests/*.js` files still pass/fail EXACTLY as
  before this change (same failures for the same still-open bugs, no new
  failures, no now-silently-passing-when-it-shouldn't).
- `tests/e2e/package.json` updated with the new dependency; `npm install`
  run so `package-lock.json` reflects it.
- Report back clearly: (a) did dynamic `import()` from CJS work cleanly, or
  did you need a workaround (e.g. converting `tests/e2e` to ESM, or a
  different interop trick) -- this affects how #920 is briefed next; (b)
  which ops in `write.ts` you found (or didn't find) a builder for, so #920
  knows the exact remaining raw-construction surface.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
