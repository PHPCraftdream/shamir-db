# Tasks #939-956 -- complete the tests/e2e query-builder migration, one file per task

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background -- repeat request, read this before touching anything

Tasks #919/#920 (2026-08-02) migrated `tests/e2e`'s wire-op construction
to `@shamir/client`'s query builders, but were EXPLICITLY SCOPED to
destructive/DDL/index ops only (chmod, create_db/repo/table/index,
drop_*, migrations) in 6 of 18 test files
(`08-admin-ddl`, `12-hmac-gate`, `14-index2-types`, `16-replication`,
`17-replication-convergence`, `18-vectors`). This was written into
those tasks' own briefs as a deliberate scope boundary, not an oversight
discovered later -- `CHANGELOG.md`'s `#917` entry already documents this
honestly.

The user has now asked TWICE for this to be finished properly -- "мы уже
делали её, но ничего не изменилось" (we already did this, but nothing
changed). This is the completion pass. Each of tasks #939-956 assigns
ONE specific file (see the task's own `TaskList` description, fetched via
`TaskGet`, for which file and any file-specific notes). Read your
assigned task's full description before starting -- it names known
residuals and exceptions specific to that file.

## What "convert" means, concretely

Every `client.execute(db, { id, queries: { alias: { <raw_wire_shape> }
} })` call currently builds `<raw_wire_shape>` as a hand-written object
literal (e.g. `{ from: table, where: {...} }`, `{ set: table, key,
value }`, `{ create_db: name }`). Replace each with the equivalent
builder call from `@shamir/client`, imported via
`const { select, filter, write, query, ddl, admin, batch, cursor, call,
subscribe } = require('@shamir/client');` (only import what you
actually use -- check `crates/shamir-client-ts/src/core/builders/`'s
files: `select.ts`, `filter.ts`, `write.ts`, `query.ts`, `ddl.ts`,
`admin.ts`, `batch.ts`, `cursor.ts`, `call.ts`, `subscribe.ts`, plus the
aggregate exports in `builders/index.ts` -- read the relevant ones for
your assigned file's op types BEFORE writing conversion code, do not
guess function signatures from memory).

For HMAC-gated destructive ops, reuse the existing
`tests/e2e/helpers/hmac.js` wrappers (`drop_table_op`,
`signerFor(client)`, etc. -- from #919's rewrite) rather than calling
`ddl.dropTable`/`admin.chmod` directly with a fresh signer, to stay
consistent with the rest of the suite.

## The one legitimate exception

A NEGATIVE test case that deliberately constructs an invalid/malformed
wire object (missing/wrong hmac, wrong type, etc.) to test server-side
REJECTION cannot be built by a builder (builders only produce valid
ops). Leave these hand-rolled, with a one-line comment stating why --
mirroring `12-hmac-gate.test.js`'s existing precedent (already correctly
left alone by #920, do not "fix" it). If you find a genuinely
builder-uncovered VALID op elsewhere, same treatment: leave it with a
one-line comment, don't block the whole file on one gap.

## What NOT to do

- Do not change any test assertion or expected behavior. Same queries,
  same expected results -- purely a construction-mechanism change.
- Do not touch any file other than the one your task assigns (residual
  sweeps of the 6 already-partially-converted files are separate tasks
  from the 12 untouched files -- stay in your lane so parallel/sequential
  work doesn't collide).
- Do not re-convert anything a prior task already correctly converted
  (for residual-sweep tasks on the 6 partially-done files) -- only fill
  the gaps.

## Verification (mandatory before considering your task done)

1. Check the CURRENT baseline first: `cd tests/e2e && node e2e.test.js`
   and note the `passed`/`failed` counts BEFORE your changes (or trust
   the count reported by the immediately-preceding task in this chain,
   if you have that context -- but verify it yourself if in doubt, don't
   assume a stale number).
2. Make your changes.
3. Re-run `cd tests/e2e && node e2e.test.js` -- the counts must be
   IDENTICAL to the baseline. Any new failure means you introduced a
   regression; find and fix it before finishing. Any DIFFERENT pass
   count (more or fewer) also means something changed behaviorally --
   investigate, don't just report the discrepancy.
4. Report the exact before/after counts in your final summary.

## Definition of done (per-file, matches your assigned task's own DoD)

- Every `queries:` block in your assigned file uses builder calls, with
  zero unjustified raw wire-object literals remaining.
- `node e2e.test.js` shows identical pass/fail counts to the
  pre-change baseline.
- Report back which builder functions you used and which (if any) ops
  you had to leave hand-rolled with justification.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention. The orchestrator commits your file alone, per task, once
verified -- do not batch multiple files' changes together even if you
can see ahead to later tasks.
