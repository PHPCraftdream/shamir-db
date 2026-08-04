# Brief — Final P1 cleanup: stale comments + RENAME INDEX `if_exists` gap

Task: #971 in the session TaskList (LAST item in the P1 release-blocker
chain — intentionally last so it documents final state after all other
fixes, not a snapshot taken mid-chain). Source:
`docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §8.8 +
§5.7, as refined by the task's own description (5 numbered items) and this
session's fresh re-verification (below). **Read this brief in full — 3 of
the review's 5 original items are ALREADY RESOLVED by earlier work this
session; do not re-investigate or "fix" them, just note them as confirmed
closed in your final report.**

## Re-verification performed before writing this brief (do not redo)

All 5 of the original task's numbered items were freshly re-checked against
the CURRENT codebase (post #957-970). Findings:

### Item 1 (table_manager_index_mgmt.rs "not implemented" comment) — NOT STALE, no action
Two distinct comments exist near lines 44-67 and 402-491 of
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`. The "not
implemented here" text (~line 65) describes the write-barrier's own
commit-path reach gap (a genuinely distinct, still-real limitation — the
barrier doesn't extend to `execute_*_tx` paths). The `backfill_index2_backend`
doc (~line 402-491) separately and correctly splits the tx-commit gap into
"Part A" (lock timing, closed) and "Part B" (ops-plan staleness, closed for
functional/INSERT by F-50/#869, confirmed live in
`crates/shamir-engine/src/tx/pre_commit.rs`). These are two different,
accurately-described gaps — **not a contradiction**. Do not touch.

### Item 2 (regular `rename_index` "register-first" stale model) — COULD NOT REPRODUCE, no action
Searched `crates/shamir-index/src/base_index/index_manager.rs`'s
`rename_index_definition`/`rename_unique_index_definition` (~line 1824-1859)
and the engine-side orchestrator `TableManager::rename_index` (~line
1030-1127 of `table_manager_index_mgmt.rs`). Neither describes an outdated
"register-first model" — the engine-side doc (~line 1088-1126) accurately
describes the CURRENT create-new-first/drop-old-second sequencing, citing
"audit A9" and the #967 (P1-2) error enrichment. No stale text matching this
item's description was found. **Do not invent something to fix here** — if
you find genuinely stale phrasing during your own read, report it, but do
not treat this item as mandatory scope.

### Item 3 (`RenameIndexOp` promises `if_exists`, doesn't have it) — CONFIRMED STALE, REAL WORK BELOW
`crates/shamir-query-types/src/admin/types/index_ops.rs:97`:
```rust
/// - Refuses when the source index does not exist (unless `if_exists = true`).
```
immediately above a struct (lines 98-108) with fields `rename_index`, `to`,
`table`, `repo` only — **no `if_exists` field**. Confirmed via grep: the
Rust builder (`crates/shamir-query-builder/src/ddl/rename_index.rs`) has no
`.if_exists()` method; the TS builder's `renameIndex()` opts
(`crates/shamir-client-ts/src/core/builders/ddl.ts:721-734`) has no
`if_exists` option either.

**Decision (made by the orchestrator, not left to you): implement the
promised feature rather than delete the doc promise.** `if_exists` for
RENAME INDEX is a natural, low-risk, additive symmetry with the ALREADY
-shipped `if_not_exists` (CREATE INDEX) and `if_exists` (DROP INDEX) — see
`DropIndexOp.if_exists` (`index_ops.rs:124-128`) and its handling in
`admin_table_index.rs::handle_drop_index` (~line 513-546) for the exact
pattern to mirror.

### Item 4 (`rekey_sorted_prefix` unconditional "atomic" claim) — ALREADY FIXED, no action
The function was renamed to `rekey_postings`
(`crates/shamir-index/src/base_index/sorted_index_manager.rs:1215`) as part
of #962 (P0-5b, commit `3d5a7785`), predating this session's #968 (P1-3)
`transact` doc audit. Its current doc (~line 1191-1214) is accurate and
explicit about non-atomicity ("Settle re-scan loop... the accepted,
F-85/#913-documented way production callers tolerate
`supports_atomic_transact() == false`"). No unconditional "atomic" claim
remains. Do not touch.

### Item 5 (`DropIndexOp.if_exists` doc vs. actual behavior) — CONFIRMED STALE, REAL WORK BELOW
`crates/shamir-query-types/src/admin/types/index_ops.rs:124-127`:
```rust
/// When `true`, dropping a non-existent index (or one whose parent
/// db/table is missing) is a silent no-op returning
/// `{"existed": false}` instead of an error.
```
This phrasing implies BOTH "non-existent index" and "missing parent
db/table" require `if_exists = true` to avoid an error. **Verified false**
by reading `admin_table_index.rs::handle_drop_index` in full (~line
496-596): the `if_exists` early-exit block (521-546) only short-circuits
BEFORE the auth/lookup calls that would `Err` on a missing DB/table
(line 559: `.ok_or_else(|| err(...))?`, line 563: `.map_err(...)?`). The
ACTUAL drop attempts (lines 571-590: `drop_unique_index`/`drop_index`/
`drop_sorted_index`/`drop_index2`) are called **regardless of
`op.if_exists`**, and the final response (line 592-595) returns
`Ok` with `"existed": @(QueryValue::Bool(removed))` **unconditionally** —
so dropping a missing index on an EXISTING table ALWAYS silently returns
`{existed: false}`, with or without `if_exists`. `if_exists`'s ONLY real
effect is on the missing-DB/missing-table case (converting what would
otherwise be an `Err` at lines 559/563 into an early `Ok{existed:false}`
at line 540-545). **Fix the doc, not the code** — this is a doc-only
correction (the behavior itself — index-missing always no-ops — is
reasonable and not the review's complaint; the complaint is that the doc
implies the WRONG variable governs it).

## Required work

### A. Doc fix — `DropIndexOp.if_exists` (item 5, doc-only)

Rewrite the doc comment at `index_ops.rs:124-127` to accurately state:
- Dropping a MISSING index on an EXISTING db/table is **always** a silent
  no-op returning `{"existed": false}` — this does NOT depend on
  `if_exists`.
- `if_exists` governs ONLY the case where the parent **db or table itself**
  is missing: with `if_exists = true`, that case ALSO silently no-ops
  (`{"existed": false}`); without it, a missing db/table is a hard `Err`
  ("Database '...' not found" / table lookup error).

Cross-reference `handle_drop_index`'s early-exit guard comment (~line
513-520, already accurate) so the two stay in sync.

### B. Implement RENAME INDEX `if_exists` (item 3, real feature)

1. **Wire type** — add to `RenameIndexOp`
   (`crates/shamir-query-types/src/admin/types/index_ops.rs:98-108`):
   ```rust
   #[serde(default, skip_serializing_if = "is_false")]
   pub if_exists: bool,
   ```
   (mirror `DropIndexOp`'s exact attribute usage — check `is_false`'s
   import/definition in this file already exists for `DropIndexOp`, reuse
   it). This is wire-backward-compatible: old messages omitting the field
   default to `false` (today's only behavior), so no existing caller's
   wire bytes change meaning.
2. **Rust builder** — add `.if_exists()` to
   `crates/shamir-query-builder/src/ddl/rename_index.rs`'s `RenameIndex`
   builder (mirror the shape of `CreateIndex::if_not_exists()` in
   `create_index.rs` — a `bool` field + a builder method setting it `true`,
   plumbed into `build()`'s `RenameIndexOp` construction).
3. **TS builder** — add `if_exists?: boolean` to `renameIndex()`'s `opts`
   in `crates/shamir-client-ts/src/core/builders/ddl.ts:721-734`, mirroring
   how `dropIndex()`'s `opts.if_exists` (line ~743) is threaded into its op.
4. **Server** — in `admin_table_index.rs::handle_rename_index` (~line
   598-650), add an early-exit guard BEFORE the existing auth/lookup calls,
   mirroring `handle_drop_index`'s pattern (~line 513-546) as closely as
   sensible: if `op.if_exists` is true, resolve the db/table (tolerating
   either being missing, same as `handle_drop_index`'s `db_opt`/`table_opt`
   pattern) and check whether `op.rename_index` (the SOURCE name) exists
   across all 4 index families (`unique_index_exists`/`index_exists`/
   `sorted_index_exists`/`index2_exists` — same 4 calls
   `handle_drop_index` already makes). If it does NOT exist (or the
   db/table doesn't exist), return early with
   `Ok(admin_result(mpack!({"renamed_index": op.rename_index.clone(),
   "existed": false})))` — do NOT call `table.rename_index(...)` in this
   case. If it DOES exist, fall through to the existing rename logic
   unchanged.
   - Also add `"existed": true` to the EXISTING success response (line
     644-649) for wire-shape consistency between the "renamed" and
     "no-op'd" outcomes (a caller can check one field either way).
   - **Do NOT change** the existing "destination name already occupied"
     rejection — `if_exists` only governs the SOURCE-missing case, per the
     doc's own (correct) wording: "Refuses when the destination name is
     already taken... Refuses when the source index does not exist (unless
     if_exists=true)." Destination-occupied stays a hard error regardless
     of `if_exists`.

### C. Tests

- Rust: extend or add a test file under
  `crates/shamir-query-builder/tests/` or `crates/shamir-db/tests/`
  (check for an existing `rename_index` test file first — reuse its
  pattern/location rather than creating a new one if one already exists)
  covering: `if_exists` on a genuinely-missing source index → `Ok` no-op;
  `if_exists` on an existing source → normal rename still works; NO
  `if_exists` on a missing source → still errors (regression guard,
  unchanged behavior); destination-occupied still errors regardless of
  `if_exists`.
- TS: extend `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts`
  (or the equivalent e2e file used for rename tests from #978/#981 —
  check `tests/e2e/tests/19-rename-describe.test.js` and this session's
  TS e2e rename tests for the established pattern) with the same coverage
  at the builder level, plus (if a live-server e2e file is the natural
  home) one live no-op-vs-error e2e case.

## Gate (MANDATORY)

```
cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-db -- --check
cargo clippy -p shamir-query-types -p shamir-query-builder -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-db --full
```
Plus, since `ddl.ts` is touched:
```
npx tsc --noEmit   # run inside crates/shamir-client-ts
npx vitest run src/core/builders/__tests__/ddl.test.ts
```

## Scope discipline

- Do NOT touch items 1, 2, 4 — they are confirmed not-stale / already-fixed
  this session. If you happen to notice something concretely wrong while
  reading nearby code, REPORT it, do not silently fix it inline (this task's
  diff should be scoped to items 3+5 only).
- Do NOT change DROP INDEX's actual runtime behavior — item 5 is a
  DOC-ONLY fix, the behavior itself (unconditional no-op for a missing
  index on an existing table) is correct and intentional.
- Do NOT touch the "destination name occupied" rejection for RENAME —
  `if_exists` only affects the source-missing case.
- This is the LAST task in the P1 chain. After this lands, do not start
  any further release-blocker work — the chain is complete once this is
  verified and committed.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Confirm items 1/2/4 were left untouched (or report anything genuinely wrong
you noticed without fixing it). Show the exact before/after of both doc
comments (item 5) and the new `if_exists` field/methods (item 3). List
every new test added and what it proves. Give exact gate command output.
