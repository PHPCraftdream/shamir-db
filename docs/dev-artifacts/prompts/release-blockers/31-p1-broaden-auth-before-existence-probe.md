# Brief — Broaden #989's auth-before-existence-probe fix to remaining handlers

Task: #995 in the session TaskList. Found by a follow-up `@oh` review
(2026-08-05) of #989's own fix. Read this brief in full — #989 already
fixed the identical pattern in exactly 2 handlers
(`handle_drop_index`/`handle_rename_index`, `admin_table_index.rs`); this
task applies the SAME reorder to the remaining 8 handlers sharing the
identical shape, verified this session by directly reading each one.

## The pattern (already fixed once — mirror it exactly)

Every handler below runs its `if_exists`/`if_not_exists` existence-check
early-exit BEFORE `authorize_access`, letting an unauthorized caller
distinguish two outcomes (a clean no-op response vs. eventually falling
through to `access_denied`) without authorization ever running for the
no-op case. #989 already fixed this exact shape in
`handle_drop_index`/`handle_rename_index` by moving `authorize_access`
to run FIRST, leaving the existence-check body otherwise byte-for-byte
unchanged. Read `admin_table_index.rs`'s CURRENT (already-fixed)
`handle_drop_index`/`handle_rename_index` as your template for the exact
reorder shape.

## Important nuance — verified this session, do not re-litigate

`resource_meta` (`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`)
returns `Ok(ResourceMeta::default())` for a genuinely-absent db/store/table
— "still open by design" — and `ResourceMeta::default() == open()` (mode
`0o777`). This means `authorize_access` ALREADY passes for ANY actor
against a resource that doesn't exist yet — reordering auth-before-probe
does NOT close a "does this db/table/repo exist at all" oracle (that's
structurally open by design, unrelated to this fix). What reordering DOES
close is the SAME thing #989 already closed for indexes: an unauthorized
caller's ability to distinguish "the if_exists/if_not_exists path fired
because the resource is genuinely absent" from "it fired because I have no
rights on an EXISTING, permission-restricted resource" — the
existing-and-restricted case is where the real leak lives. Keep this
framing in mind when writing test assertions (below) — do not write a test
asserting "unauthorized + nonexistent resource → access_denied" as if that
were a NEW guarantee this fix provides for the resource-doesn't-exist case;
it's the EXISTING-restricted-resource case that actually changes behavior.

Wait — reread #989's own tests (`sec1_ddl_gate_e2e.rs`,
`drop_index_if_exists_denies_unauthorized_on_missing_index`): that test DOES
assert `access_denied` for an unauthorized caller against a MISSING index,
inside an EXISTING (but access-restricted) TABLE. The db/store/table itself
existed and was restricted; only the INDEX (a sub-resource) was missing.
So the correct framing for #989 (and this task) is: authorization is keyed
to the RESOURCE PATH the `authorize_access` call actually checks (e.g. the
parent table's `Write`/`Delete` right), which is INDEPENDENT of whether the
specific thing being created/dropped (index/table/db/repo/function/
validator) exists. Re-verify for EACH handler in this task exactly what
resource path its `authorize_access` call checks, and construct your test's
"restricted" setup against THAT resource path specifically (not
necessarily the same resource being created/dropped).

## Handlers to fix (8 total, verified this session)

1. **`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`**:
   - `handle_create_table` (~line 14-91): existence check (`if
     db.has_table(...)`, ~line 32-50, `if_not_exists`) runs before
     `authorize_access` (~line 51-58, checks `ResourcePath::store(db,
     repo)`, `Action::Create`).
   - `handle_drop_table` (~line 93+): `if_exists` early-exit (~line
     110-122) runs before `authorize_access` (~line 124-131, checks
     `ResourcePath::table(db, repo, table)`, `Action::Delete`). **Extra
     care**: this function ALSO has a reverse-FK drop guard AFTER the auth
     call (~line 133+) — do not reorder that, only move the existence
     check relative to auth.
2. **`crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs`**:
   - `handle_create_db` (~line 16+): existence/`if_not_exists` check
     before `authorize_access(&ResourcePath::Root, Action::Create)`
     (~line 55).
   - `handle_drop_db` (~line 68+): `if_exists` check (~line 81) before
     `authorize_access` (~line 89).
   - `handle_create_repo` (~line 135+): existence/`if_not_exists` check
     (~line 188-191) before `authorize_access` (~line 209).
   - `handle_drop_repo` (~line 297+): `if_exists` check (~line 310)
     before `authorize_access` (~line 324).
3. **`crates/shamir-db/src/shamir_db/execute/admin_function.rs`**:
   - `handle_drop_function` (~line 99+): `if_exists` check (~line 121)
     before `authorize_access` (~line 129).
4. **`crates/shamir-db/src/shamir_db/execute/admin_validator.rs`**:
   - `handle_drop_validator` (~line 78+): `if_exists` check (~line 100)
     before `authorize_access` (~line 114).

For EACH handler: verify these line numbers against the CURRENT file
content yourself (this session's investigation, not guaranteed to be
byte-exact after any intervening change) before editing. Confirm
`handle_create_function`/`handle_create_validator` do NOT have the same
shape (this session's investigation found they don't have an
`if_not_exists`-gated existence check preceding their auth calls — but
verify this yourself; if you find one, fix it too and note the deviation
in your report).

## Required fix — same reorder pattern for all 8

Move each handler's `authorize_access(...).await.map_err(err_access)?;`
call to run BEFORE its existence-check early-exit block, leaving the
existence-check body's internal logic (the four-family-lookup style
checks, the no-op response shape) byte-for-byte unchanged — exactly
mirroring #989's own diff shape. Do NOT change what resource
path/`Action` each `authorize_access` call checks — only its POSITION
relative to the existence check moves.

## Required tests

Extend `crates/shamir-db/tests/sec1_ddl_gate_e2e.rs` (the file #989 added
its own 4 tests to) with one pair of tests per handler (8 handlers × 2 =
16 new tests, OR fewer if you find a handler genuinely doesn't need a
"missing + unauthorized" case — use judgment, but default to full
coverage matching #989's own pattern):
- Unauthorized actor + the target resource (table/db/repo/function/
  validator) does NOT exist, but the RESOURCE PATH `authorize_access`
  actually checks IS restricted → must get `access_denied` (the actual
  fix — was a silent no-op before).
- Unauthorized actor + the target resource DOES exist (still restricted)
  → must ALSO get `access_denied` (regression guard — this already worked
  before the fix).

Follow #989's own test helpers (`assert_access_denied!`, `setup`,
`restrict_table`, `Actor::User(OTHER)`) — reuse or extend them as needed
for db/repo/function/validator-level restriction rather than only
table-level, since some of these handlers check `ResourcePath::Root`/
`ResourcePath::store`/etc., not `ResourcePath::table`. Check
`crates/shamir-types/src/access.rs` for the full `ResourcePath` variant
set and how each is restricted in existing tests, if `restrict_table`
doesn't cover the needed variant.

## Scope discipline

- Do NOT touch any handler not listed above.
- Do NOT change what resource/Action each `authorize_access` call
  checks — reorder only.
- Do NOT touch `handle_drop_table`'s reverse-FK guard logic — it stays
  exactly where it is, after auth.
- If you find `handle_create_function`/`handle_create_validator` (or any
  OTHER handler in these 4 files) has the same shape and was missed by
  this session's investigation, fix it too and clearly flag the
  deviation — do not silently expand scope without reporting it.

## Gate (MANDATORY)

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db --full
```

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit files and run read-only/test/gate
commands.

## What to report back

List every handler fixed, its exact before/after ordering (a short diff
excerpt per handler, or one representative example plus a summary table
for the rest). List every new test added and which handler/scenario each
covers. Explicitly confirm whether `handle_create_function`/
`handle_create_validator` needed fixing too (and if so, that you fixed
them). Give exact gate command output.
