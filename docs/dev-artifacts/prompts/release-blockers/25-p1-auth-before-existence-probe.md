# Brief — Security-adjacent: auth must run BEFORE the if_exists existence probe

Task: #989 in the session TaskList. Found by an adversarial `@oh` review of
the just-completed #957-971 wave (2026-08-04) while checking #971's new
RENAME INDEX `if_exists` feature. Read this brief in full.

## The gap — confirmed by direct read, not just the review's claim

`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` has TWO
handlers with the identical shape:

- `handle_drop_index` (~line 496-596): the `if_exists` early-exit guard
  (~line 513-546) runs BEFORE `authorize_access` (~line 548-555). This is
  PRE-EXISTING (not introduced this session).
- `handle_rename_index` (~line 598-680ish, added by #971): the `if_exists`
  early-exit guard (~line 619-647) ALSO runs BEFORE `authorize_access`
  (~line 648ish) — faithfully mirroring `handle_drop_index`'s existing
  shape, per #971's own brief instruction to mirror the pattern.

**The consequence:** an authenticated but UNAUTHORIZED caller (no `Write`
right on the table/index resource) sending a `drop_index` or `rename_index`
request with `if_exists: true` gets:
- `Ok({..., "existed": false})` — a clean, silent no-op — when the
  index/table/db does NOT exist, with `authorize_access` NEVER CALLED.
- `access_denied` (from `authorize_access`) only when it DOES exist and the
  code falls through past the guard.

This is a pre-auth existence oracle: an unauthorized caller can distinguish
"exists" from "doesn't exist" for indexes, tables, and databases they have
no right to even query, by toggling `if_exists` and reading which of the two
distinguishable outcomes comes back. It also triggers `db.get_table(...)`
(potential lazy `TableManager` instantiation) before any authorization check
runs at all.

Not a NEW bug introduced by #971 — `handle_rename_index` faithfully mirrors
`handle_drop_index`'s pre-existing shape. This fix closes it in BOTH places
at once, since they share the identical defect.

## The fix — reorder only, no behavior change for any authorized caller

For BOTH `handle_drop_index` and `handle_rename_index`: move the
`if op.if_exists { ... }` early-exit block to run AFTER
`authorize_access(...).await.map_err(err_access)?;` succeeds, instead of
before it. Do not change anything ELSE about either block's internal logic
— the existence-check body (its own independent `get_db`/`get_table`
resolution, the four-index-family existence check, the early-return shape)
stays exactly as-is; only its POSITION relative to the auth call moves.

Concretely, in each function the new top-to-bottom order becomes:
1. Local closures (`err`, `err_code`, `err_access`) — unchanged position.
2. `self.shamir.authorize_access(...).await.map_err(err_access)?;` — moved
   to run FIRST (before the if_exists block).
3. The `if op.if_exists { ... existence check ... early return }` block —
   now runs second, only reached once the caller is confirmed authorized.
4. The rest of the function (the actual drop/rename, unchanged) —
   continues exactly as before.

Verify after making the change: an authorized caller sees IDENTICAL
behavior to before (this is a pure reordering — same result for anyone who
passes auth, on both the exists and doesn't-exist branches). Only an
UNAUTHORIZED caller's behavior changes: they now get `access_denied` in
EVERY case (existing or not), never a silent `{"existed": false}` no-op
before auth ever ran.

## Required tests

Check whether an existing e2e/integration test file already covers
`if_exists` + a permission/ACL scenario for either DROP or RENAME INDEX
(search `tests/e2e/tests/` and `crates/shamir-db/tests/` for
`if_exists`-adjacent ACL tests first — reuse the pattern/location if one
exists rather than creating a new file). Add, for BOTH `drop_index` and
`rename_index`:
- An UNAUTHORIZED caller (no `Write` on the resource) sending
  `if_exists: true` against a NON-existent index → must get `access_denied`
  (NOT a silent `{"existed": false}` success).
- An UNAUTHORIZED caller sending `if_exists: true` against an EXISTING
  index → must also get `access_denied` (this already worked before the
  fix — regression guard proving the fix didn't change this case).
- An AUTHORIZED caller's existing `if_exists` behavior (both exists and
  doesn't-exist cases) is UNCHANGED — reuse/extend whatever tests already
  cover #971's `if_exists` feature and #977's `DROP INDEX ... IF EXISTS`
  coverage (`tests/e2e/tests/08-admin-ddl.test.js`,
  `crates/shamir-db/tests/rename_index_e2e.rs`) rather than duplicating
  them — just confirm they still pass unchanged.

## Scope discipline

- Do NOT change the existence-check logic itself (the four-index-family
  check, the `if_not_exists`/no-op response shape) — only its position
  relative to `authorize_access`.
- Do NOT touch any other handler in this file — scope is exactly
  `handle_drop_index` and `handle_rename_index`.
- Do NOT add a new authorization mechanism or right — this uses the SAME
  `authorize_access` call already present in both functions, just moved
  earlier.

## Gate (MANDATORY)

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db --full
```
If you add/extend a JS e2e test in `tests/e2e/`, also run that suite's
existing invocation (check `tests/e2e/package.json` / README for the exact
command used by prior briefs this session, e.g. #977's).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit files and run read-only/test/gate
commands.

## What to report back

Show the exact before/after ordering for both functions (a short diff
excerpt is enough). List every new test added and confirm each: (a) fails
against the OLD ordering (reproduces the oracle) and (b) passes against the
NEW ordering — for at least one of the two handlers, actually verify this
fail/pass cycle yourself (temporarily revert, run, restore, run again) the
same way #987 was verified, rather than reasoning about it only in the
abstract. Give exact gate command output.
