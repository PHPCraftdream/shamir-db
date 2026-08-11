# Brief 79 — #1078 (LOW): `DropIndexOp::unique` is dead for resolution but still perturbs the HMAC; `if_exists` asymmetry vs `DROP TABLE`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

This brief covers TWO independent defects in the same two files
(`crates/shamir-query-types/src/admin/types/index_ops.rs`,
`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`'s
`handle_drop_index`). Land them as two logically separate groups of changes
(can be one commit or two, your call), but implement BOTH — do not stop
after the first.

## Defect 1 — `unique` field is dead for resolution, still perturbs the HMAC signature

**Already investigated and confirmed directly against the code** (not just
the review's claim — re-verify yourself before touching anything):

- `crates/shamir-query-builder/src/ddl/drop_index.rs:31-43`'s `.unique()`
  builder method doc comment already admits: *"This field is now
  informational-only and used only for HMAC canonical input generation...
  Setting this incorrectly does not affect which index is dropped, only
  changes the bytes signed into the HMAC."*
- `crates/shamir-server/src/db_handler/admin.rs:666-675` is the ONLY
  non-HMAC, non-serde-derive place `DropIndexOp::unique` is read anywhere in
  the Rust workspace (confirmed via
  `grep -rn "op\.unique\b" crates/shamir-server` and equivalent greps
  elsewhere) — it feeds `canon::canonical_drop_index(..., op.unique)`.
  `handle_drop_index` in `admin_table_index.rs` resolves the family
  ENTIRELY from the catalog (`is_regular`/`is_unique`/`is_sorted`/
  `is_index2`, computed via `table.index_exists`/`unique_index_exists`/
  `sorted_index_exists`/`index2_exists`) — `op.unique` never enters that
  logic (see the comment at `admin_table_index.rs:741-743`: *"the server
  now resolves the index family from the catalog, not from the client's
  `op.unique` hint"*).
- `crates/shamir-query-types/src/hmac.rs:118-134`
  (`canonical_drop_index`) — the canonical bytes are
  `b"drop_index\0<db_in_use>\0<repo>\0<table>\0<index>\0<unique:0|1>"`. This
  is the ONLY thing `unique` still affects.

**Failure scenario:** the same logical "DROP INDEX X ON T" op has two
different valid HMAC tags depending on a flag that means nothing anymore.
An operator who signs the canonical string with `unique=false` while a
client wrapper (or a TS caller using a different default) sends
`unique=true` gets a spurious `hmac_mismatch` — with no way to see WHY from
the error message, since the flag has no other observable effect.

**Fix: remove `unique` entirely — from the wire type, the builder, the HMAC
canonical input, both languages.** Do not just `#[deprecated]`-annotate it;
alpha (`0.1.0-alpha.1`) is the explicit breaking-change window (per this
task's own framing and the precedent set by #1075's typed-`CreateIndex`
breaking change this same session) and a field that does *nothing* but
perturb a signature is worse to leave around than to remove.

Required changes (Rust):

1. `crates/shamir-query-types/src/hmac.rs`:
   - `canonical_drop_index`: drop the `unique: bool` parameter and the
     `unique_byte` line; canonical bytes become
     `b"drop_index\0<db_in_use>\0<repo>\0<table>\0<index>"` (same shape as
     `canonical_drop_table`).
   - Update the module-doc table entry for `drop_index` (line ~33) to match.
2. `crates/shamir-query-types/src/admin/types/index_ops.rs`:
   - Remove `pub unique: bool` from `DropIndexOp` (currently line 137, with
     its `#[serde(default)]` attribute).
   - Update the struct doc comment (lines 128-131) — the `hmac` requirement
     line currently quotes the OLD canonical format string; update it to
     match the new one.
3. `crates/shamir-server/src/db_handler/admin.rs:666-675`: drop the
   `op.unique` argument from the `canonical_drop_index(...)` call.
4. `crates/shamir-query-builder/src/ddl/drop_index.rs`:
   - Remove the `unique: bool` field from the `DropIndex` struct, the
     `unique: false` initializer in `drop_index(...)`, the `.unique()`
     builder method entirely, and the `unique: self.unique` line in
     `build()`.
5. Fix any other `op.unique` / `DropIndexOp { unique: ..., .. }` construction
   site the removal breaks — the compiler will find them; do not rely on
   `grep` alone, run `cargo build --workspace` and follow every error.

Required changes (TypeScript, `crates/shamir-client-ts/src/core/`):

6. `hmac.ts`'s `canonicalDropIndex` (~line 113-128): drop the `unique:
   boolean` parameter and the `unique ? '1' : '0'` element from the
   `joinNull([...])` call.
7. `builders/ddl.ts`'s `dropIndex` (~line 1058-1086): drop the `unique?:
   boolean` option from `opts`, the `const unique = opts?.unique ?? false;`
   line, the `unique` argument to `canonicalDropIndex(...)`, and the
   `if (unique) op.unique = true;` line. Update the doc comment above it
   (currently describes the now-removed field as "informational-only" — that
   framing no longer applies since there's nothing left to be informational
   about).
8. Check `DropIndexOp`'s TypeScript wire-type definition (wherever it's
   declared — likely near the other `*Op` types the builders reference) and
   remove the `unique?: boolean` field there too if present.
9. `npx tsc --noEmit` (in `crates/shamir-client-ts`) after your edits — the
   compiler will find any other TS call site.

Required test updates — this is the bulk of the work, read carefully:

10. **`crates/shamir-db/tests/ddl_wire_e2e/drop_index_unified_resolution.rs`**
    (the #1025 test suite, 8 scenarios) — scenarios 2, 4, 6 currently call
    `.unique()` explicitly on the DROP builder (`ddl::drop_index(...).unique()`)
    as their whole premise ("client sends a WRONG unique hint, server still
    resolves correctly via the catalog"). Since `.unique()` no longer
    exists, this premise is gone. Do NOT delete these tests — they still
    have real regression value (proving catalog-based resolution works
    correctly for both regular and unique indexes, unrelated to any client
    hint). Instead: remove the now-nonexistent `.unique()` calls, and
    rename/reword the test names, doc comments, and inline comments that
    reference "mismatched flag" / "wrong value" / "unique defaults to
    false" — none of that framing is accurate anymore since there's no flag
    to mismatch. Scenarios 1&2, 3&4, 5&6 will become structurally identical
    in shape after this (both just "drop resolves via catalog, no flag
    involved") — keep them as separate tests (one for the unique-index case,
    one for the regular-index case) rather than deleting the "duplicate"
    since they exercise different index families, just rename to reflect
    what they actually test post-fix (e.g. `drop_unique_index_via_catalog_resolution`,
    `drop_regular_index_via_catalog_resolution`). Scenario 8 (cross-family
    collision) doesn't touch `.unique()` on drop — leave as-is.
11. **`crates/shamir-query-types/src/tests/hmac_tests.rs`** — find every
    `canonical_drop_index(...)` call and drop the `unique` argument; verify
    any hardcoded expected-byte-vector assertions are updated to match the
    new (shorter) canonical bytes.
12. **`crates/shamir-server/tests/hmac_gate.rs`** — same: find
    `canonical_drop_index` usage, update signature/expected bytes. Check
    whether any test specifically exercises "different `unique` value →
    different HMAC tag" for drop_index (mirroring what
    `tests/e2e/tests/12-hmac-gate.test.js:136` does — see item 14 below) —
    if so, that test's premise is now false and needs the same treatment
    as item 14.
13. **TypeScript**: `crates/shamir-client-ts/src/core/__tests__/hmac.test.ts`
    and `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts` —
    find every `canonicalDropIndex(...)` / `dropIndex(...)` call passing a
    `unique` argument/option and update.
14. **`tests/e2e/tests/12-hmac-gate.test.js`** — the test
    `'drop_index unique=true requires its own tag flavour'` (line ~136-160)
    exists SPECIFICALLY to prove tampering with `unique` post-signing breaks
    the HMAC check. That premise is now false by design (removing `unique`
    from the canonical input is the whole point of this fix) — this test
    must be REMOVED, not adapted (there's no meaningful "different tag
    flavour" left to test for this op; the surrounding suite's OTHER
    `drop_index` HMAC tests, if any, that don't depend on `unique` should
    stay). Check `tests/e2e/helpers/hmac.js`'s `drop_index_op` (line
    108-110) — it forwards `opts` (including `unique`) straight to
    `ddl.dropIndex(...)`; once `dropIndex`'s `unique` option is gone, any
    caller still passing `{ unique: true }` in `opts` will just have it
    silently ignored by TS's structural typing UNLESS you also remove the
    option from the `dropIndex` TS signature (item 7 above) — after that
    removal, passing `unique` in an object literal to `dropIndex(...)`
    becomes a TS compile error (excess property check on a literal), which
    is what you want: it will force you to find and fix every remaining
    caller.

## Defect 2 — `if_exists: false` semantics are inconsistent with `DROP TABLE`

**Already investigated and confirmed directly against the code:**

`crates/shamir-query-types/src/admin/types/table_ops.rs:44-47`
(`DropTableOp::if_exists`) — `true` → missing table (or missing db/repo) is
a silent no-op; `false` (default) → a missing table is a hard `Err`
("Database '...' not found" style). This is the CONTRACT users expect from
an `if_exists` flag on any DROP.

`crates/shamir-query-types/src/admin/types/index_ops.rs:142-153`
(`DropIndexOp::if_exists`) — the doc comment ALREADY admits the asymmetry:
*"Governs ONLY the case where the parent db or table itself is missing...
dropping a non-existent index on an existing db/table is ALWAYS a silent
no-op ... regardless of this flag."* So `if_exists: false` on `DROP INDEX`
does NOT behave like `DROP TABLE`'s `if_exists: false` — a missing INDEX
(as opposed to a missing db/table) never errors, no matter what the flag
says.

`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`'s
`handle_drop_index` — traced the control flow precisely, confirm this
yourself before editing:

- Lines ~605-626: `if op.if_exists { ... }` block. Checks db+table+index
  existence TOGETHER (via `table_opt`, defaulting `index_exists` to `false`
  if `table_opt` is `None`) and returns EARLY with `{"existed": false}` the
  moment ANY of {db, table, index} is missing. This means: whenever
  `op.if_exists` is `true`, this block ALWAYS short-circuits before the rest
  of the function runs, for EVERY missing-anything case (db, table, OR
  index) — not just the db/table case the doc comment describes.
- Lines ~628-635 (only reached when `op.if_exists` is `false`, since the
  block above already exited otherwise): `db.get_db(...)` /
  `db.get_table(...)` — correctly hard-errors if db/table missing. This
  matches `DROP TABLE`'s contract already.
- Lines ~691-719 (the `kind` determination `if/else` chain: is_unique →
  is_index2 → is_regular → is_sorted → else): the final `else` arm
  (~709-719) fires when NONE of the four families match — i.e. the table
  exists but no index of that name exists in any family. **Because block 1
  above already exited whenever `op.if_exists` was `true`, this `else` arm
  is ONLY EVER reached when `op.if_exists` is `false`** — trace this
  yourself to confirm, it is not a hypothetical. This arm currently does
  `return Ok(admin_result_with_op_id(mpack!({"dropped_index": ..., "existed":
  false}), op_id))` — i.e. it returns SUCCESS even though `if_exists` is
  `false`. This is the exact asymmetry: `DROP TABLE` with `if_exists: false`
  on a missing table → hard error; `DROP INDEX` with `if_exists: false` on a
  missing index (existing table) → silent success.
- There is ALSO a second, structurally UNREACHABLE "no family matches"
  branch inside the dispatch block further down (~lines 761-771, the
  `else { false }` arm of the `let removed = if is_regular {...} else if
  ... } else { false };` chain) — unreachable because by the time execution
  reaches this dispatch, the `kind` block above has ALREADY returned early
  for the no-match case. Confirm this reachability claim yourself (read the
  full function top to bottom) before touching it — if your reading differs
  from this brief's, trust your own reading and say so in your report.

**Fix:** change the `else` arm at ~line 709-719 (the one reached only when
`if_exists` is `false` and no family matches) from returning
`Ok({"existed": false})` success to returning a hard error, mirroring
`DROP TABLE`'s `"Database '...' not found"` style — something like
`Err(err(format!("index '{}' not found on table '{}'", op.drop_index,
op.table)))`. No `if op.if_exists` check is needed inside this arm itself
(re-verify this claim — if you find a path where this arm CAN be reached
with `if_exists: true`, the fix needs an explicit check instead, and your
report must say so and why).

For the confirmed-unreachable dispatch-level `else { false }` arm
(~761-771): replace it with `unreachable!("...")` with a comment explaining
why (matches CLAUDE.md's sanctioned use of `unreachable!()` for invariant
violations that mean a programmer bug) — do NOT leave it as silently
returning `false`, since that comment currently references "the client's
unique hint" which no longer exists after Defect 1's fix. If your own
reachability analysis disagrees and finds a live path here, do NOT force an
`unreachable!()` — report the discrepancy instead and leave it functioning,
with an updated (accurate) comment.

Update `DropIndexOp::if_exists`'s doc comment (`index_ops.rs:142-153`) to
describe the NEW unified semantics — mirror `DropTableOp::if_exists`'s doc
comment shape (missing db/repo/table/index, ALL governed uniformly by this
one flag).

### Required tests for Defect 2

New test(s) in `crates/shamir-db/tests/ddl_wire_e2e/drop_index_unified_resolution.rs`
(or a sibling file if that one is getting crowded — your call):

1. `DROP INDEX` with `if_exists: false` (the default — no `.if_exists()`
   call) on a table that EXISTS but has NO index of that name → must now
   return an `Err` (was previously `Ok({"existed": false})`). Assert on the
   error, not just `.is_err()` — check the message mentions the index name
   and/or "not found".
2. `DROP INDEX` with `if_exists: true` on the same missing-index scenario →
   still succeeds with `{"existed": false}` (unchanged behavior — this is
   the regression guard proving you didn't break the `if_exists: true`
   path).
3. **Mandatory revert-and-check** for test 1: temporarily revert your
   `admin_table_index.rs` change, confirm test 1 goes GREEN incorrectly
   (i.e. it currently returns `Ok` where you're asserting `Err` — flip the
   assertion temporarily or just note the pre-fix `Ok` result in your
   report), then restore the fix and confirm test 1 passes as written.
   Report this explicitly.

Also **audit every existing test in the repo that currently drops a
non-existent index without `.if_exists()` and expects success** — your
Defect 2 fix will break any such test (that was previously relying on the
buggy no-op behavior). Search broadly
(`grep -rn "drop_index" crates/*/tests crates/*/src/**/tests -l` and read
each hit) — do not assume the list in this brief's earlier file inventory
is exhaustive for this specific behavior change; it was compiled for the
`unique` field, not for `if_exists` semantics, so it may miss tests that
never touch `unique` at all. Fix each discovered test to either add
`.if_exists()` (if that's what the test actually intends) or accept the new
error (if the test was accidentally relying on the lenient behavior).

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db --full
./scripts/test.sh -p shamir-query-types
./scripts/test.sh -p shamir-query-builder --full
./scripts/test.sh -p shamir-server --full
```

For TypeScript (`crates/shamir-client-ts`):
```
npx tsc --noEmit
npx vitest run
```

For the e2e JS suite touched by item 14
(`tests/e2e/tests/12-hmac-gate.test.js`) — check this repo's established
way to run it (look for a script/README in `tests/e2e/`) and run at least
that one file's suite; report the actual command and output. If it requires
a live server and that's impractical in your environment, say so explicitly
in your report rather than silently skipping — do not claim a pass you
didn't observe.

Paste the actual final summary line from every command above — literal
output, not a paraphrase. List every test you added/touched/removed by name
with individual pass/fail status, and the outcome of the mandatory
revert-and-check for Defect 2's new test. If anything fails, fix it before
reporting done. Also explicitly state, for each of the two "trace this
yourself" reachability claims in this brief (the `if_exists`-early-exit
implies the `else` arm at ~709-719 is if_exists-false-only; the dispatch-level
`else { false }` at ~761-771 being unreachable), whether your own reading
confirmed or contradicted the brief's claim, and what you did in response.
