# Brief — #990 follow-up: fix the JS e2e test regression it caused

Task: #996 in the session TaskList. Found by a follow-up `@oh` review
(2026-08-05) of the #987/#989/#990/#991 fixes. Read this brief in full.

## The regression — confirmed by direct read

#990 (`542d03c3`) added a client-side `include`-without-`sorted` check to
the TS builder's `createIndex()`
(`crates/shamir-client-ts/src/core/builders/ddl.ts`). Before #990, this
combination was NOT checked client-side at all — the op was constructed and
sent to the server, which rejected it there.

`tests/e2e/tests/14-index2-types.test.js`'s test
`'covering: include on non-sorted index is rejected'` (~line 474-494) was
written for the OLD (server-round-trip) behavior:

```js
const err = await assertThrows(() =>
  client.execute(db, {
    id: 'mk-bad',
    queries: {
      i: ddl.createIndex('bad_include', 't', [['score']], {
        include: [['label']],
      }),
    },
  }),
);
assert(
  /include is only valid for sorted indexes/i.test(err.message),
  `expected include-rejection error, got: ${err.message}`,
);
```

`assertThrows` (`tests/e2e/helpers/runner.js`) catches SYNCHRONOUS throws
too. #990's new client-side check throws synchronously, inside the
`ddl.createIndex(...)` call itself — so `err` is now that client-side
error:

```
createIndex: `include` is only valid for sorted indexes; call sorted: true
before include, or drop the include option (server rejects include without
sorted — see admin_table_index.rs)
```

The regex `/include is only valid for sorted indexes/i` does NOT match
this — the backtick before `include` (`` `include` is only valid... ``)
breaks the expected unbroken phrase `include is only valid`. This test will
FAIL the moment it actually runs against a client build that has #990's
change.

**Currently masked**: `crates/shamir-client-ts/dist/core/builders/ddl.js`
is a STALE build artifact that predates #990 — it still has no `include`
validation, so the test currently exercises the OLD server-round-trip path
and passes. The bug surfaces the instant `npm run build:client-ts` (part of
the e2e suite's own build chain, see `tests/e2e/package.json`) rebuilds
`dist/` and picks up #990's change.

**Secondary loss**: once the TS builder rejects `include`-without-`sorted`
client-side, this specific test can no longer reach the SERVER's own
pre-existing check (`admin_table_index.rs`'s `!op.include.is_empty() &&
!op.sorted` rejection) via the live wire path at all — it was the only
e2e/integration coverage of that server-side check.

## Required fix — two parts

### 1. Fix the JS e2e test's regex

In `tests/e2e/tests/14-index2-types.test.js`'s
`'covering: include on non-sorted index is rejected'` test:
- Loosen the regex to `/include.*only valid for sorted indexes/i` (matches
  regardless of the backtick).
- Update the test's own inline comment — it currently says the assertion
  targets `admin_table_index.rs`'s server-side check; since this test now
  exercises CLIENT-side validation (the TS builder throws before any wire
  round-trip), correct the comment to say so. This is still a legitimate,
  valuable test — just of a different layer than it originally was.

### 2. Restore server-side e2e coverage for the SAME check

Add a NEW test to `crates/shamir-db/tests/create_index_validation_e2e.rs`
(already exists, extended by #970/#990) proving the SERVER still rejects
`include`-without-`sorted` via the live wire pipeline:

- Use `CreateIndex::build()` (the LENIENT, non-validating path —
  `IntoBatchOp for CreateIndex` at `create_index.rs` uses `build()`, not
  `try_build()`) to construct the invalid op, since `try_build()` would
  reject it client-side (Rust-side) before it ever reaches the server —
  you need the op to actually arrive at the server unvalidated to prove the
  server's OWN check fires.
- Follow the exact pattern of the file's existing `exec_create` helper
  and other `server_rejects_*` tests (e.g. `server_rejects_empty_fields`,
  `server_rejects_include_on_non_btree` which #990 already added) — but
  construct the op via `.build()` explicitly rather than through
  `exec_create`'s builder-argument helper if that helper calls
  `try_build()` internally (check this — if `exec_create` uses
  `try_build()`, you'll need a small variant or direct construction for
  this specific test).
- Assert the server rejects with a message mentioning "include" and
  "sorted" (mirroring the existing test's assertion style).

## Verification requirement — do not skip

For the JS e2e fix specifically: **actually rebuild the TS client and run
the real e2e test**, not just reason about the regex change abstractly —
this is exactly the class of gap that caused the original miss (the fix
was verified only via vitest + Rust tests, never the JS e2e suite that
this test lives in). Steps:
```
cd crates/shamir-client-ts && npm run build   # or the e2e suite's own
                                                # build:client-ts step —
                                                # check tests/e2e/package.json
cd ../../tests/e2e && npm test -- 14-index2-types
```
(Check `tests/e2e/README.md` / `package.json` for the exact commands prior
briefs this session used for JS e2e runs, e.g. #977's brief, and follow
that exact invocation — do not guess a different one.) Report the actual
pass/fail output.

## Scope discipline

- Do NOT touch any other test in `14-index2-types.test.js` or
  `create_index_validation_e2e.rs` beyond what's needed here.
- Do NOT revert or weaken #990's client-side `include` validation — it is
  correct and intentional; this task only fixes the test that didn't
  anticipate it.
- Do NOT touch the TS builder's validation logic itself.

## Gate (MANDATORY)

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db --full
```
Plus the JS e2e verification described above (this IS the gate for the JS
side — there is no separate "vitest-only" substitute this time).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Show the exact before/after of the JS test's regex + comment. Show the new
Rust test in full. Give the ACTUAL output of running the rebuilt-client JS
e2e test (not just an assertion that it "should" pass). Give exact Rust
gate command output.
