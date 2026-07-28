# Brief for #872 (P1) — add index2/sorted-index support to `DROP INDEX`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace (a database engine). Investigated
during F-50's Step 3 scoping this session: `DROP INDEX` currently has NO
support for index2 backends (functional/fts/vector) or the legacy sorted-
index path at all — only the original regular/unique btree indexes.

**Read `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` in
full first** (already read this session — confirmed below, don't
re-derive, but verify it yourself before writing code).

Confirmed findings:
- `handle_drop_index` (`:422-497`) only ever calls `table.drop_unique_index()`
  or `table.drop_index()` — both legacy-only (`table_manager_index_mgmt.rs:532-544`,
  which call `index_manager.drop_index`/`drop_unique_index`). It NEVER
  touches `index2_registry` or `sorted_indexes`.
- `DropIndexOp` (`crates/shamir-query-types/src/admin/types/index_ops.rs:115-129`)
  has no `index_type` discriminant — just `drop_index: String`,
  `table: String`, `unique: bool`, `repo`, `hmac`, `if_exists`. So
  resolution must be by NAME across whichever index mechanism actually
  has it (legacy regular, legacy unique, sorted, or index2) — there's no
  wire hint telling the handler which one to look in.
- The `if_exists` early-exit (`:440-462`) only checks
  `unique_index_exists`/`index_exists` (legacy) — an index2 or sorted
  index of the same name would be reported as "does not exist" even
  though it does, which is also wrong and must be fixed alongside the
  drop itself.
- The ONLY existing index2 removal path today is `DROP TABLE ... CASCADE`
  (`admin_table_index.rs:221-228`): it iterates
  `table.index2_registry().all_backends()` and calls `remove_by_id` —
  but that's only safe because the WHOLE table (and its metadata/data) is
  being destroyed in the same operation, so it doesn't bother persisting
  the index2 registry's reduced state or cleaning up posting entries
  (they die with the table). A standalone `DROP INDEX` on an index2
  backend needs BOTH of those things done properly, since the table
  keeps living.
- `IndexBackend::drop_all()` (`crates/shamir-index/src/backend.rs:79`)
  already exists and is presumably the right primitive to clean up a
  backend's posting entries before removing it from the registry —
  confirm its actual behavior by reading its impls (functional/fts/vector
  backends) before relying on it.
- `save_index2_metadata` (`crates/shamir-index/src/persistence.rs:53-71`)
  re-derives `PersistedIndexes` from the LIVE registry's
  `all_descriptors()` every time it's called — so calling it AFTER
  `registry.remove_by_id(id)` naturally persists the removal (no special
  "delete" persistence path needed, just re-save).
- `SortedIndexManager::drop_index` (`crates/shamir-index/src/legacy/sorted_index_manager.rs:222+`,
  read the whole method) already exists and is already correctly wired for
  the CASCADE-drop-table path (`admin_table_index.rs:212-220`) — but
  standalone `DROP INDEX <sorted-name>` is ALSO not reachable today for
  the same "resolution by name only checks legacy regular/unique" reason.
  Confirm whether `TableManager` already exposes a public wrapper for
  `sorted_indexes().drop_index()` by name (may need one, mirroring
  `drop_index`/`drop_unique_index`'s existing `intern_string` + delegate
  pattern at `table_manager_index_mgmt.rs:532-544`).

## What to implement

1. **A name-resolution helper (or inline logic) in `handle_drop_index`**
   that tries, in order: legacy unique (if `op.unique`), legacy regular,
   sorted index, index2 backend — using each mechanism's own name-lookup
   (`unique_index_exists`/`index_exists`/`sorted_indexes().iter_indexes()`
   name match/`index2_registry().get_by_name()`). Fix the `if_exists`
   early-exit (`:440-462`) to check all four, not just the two legacy
   ones.

2. **A `TableManager::drop_sorted_index(name: &str) -> DbResult<bool>`**
   (or similarly named) public wrapper, mirroring `drop_index`/
   `drop_unique_index`'s existing shape (intern the name, delegate to
   `self.sorted_indexes.drop_index(name_id)`), if one doesn't already
   exist — confirm first, don't duplicate if it's already there under a
   different name.

3. **A `TableManager::drop_index2(name: &str) -> DbResult<bool>`** (new)
   that: resolves the backend by name via `index2_registry().get_by_name()`
   (return `Ok(false)` if absent), calls `backend.drop_all()` to clean up
   its posting entries, calls `index2_registry().remove_by_id(backend.descriptor().id)`,
   then calls `save_index2_metadata(&self.index2_registry, &self.info_store)`
   (or whatever the exact accessor names are — check `create_index_v2`'s
   own call to `save_index2_metadata` for the right argument shapes) to
   persist the removal. Return `Ok(true)`.

4. **Wire all of this into `handle_drop_index`**, trying legacy first (to
   preserve existing behavior/error messages for the common case
   unchanged), then sorted, then index2, returning the first match's
   result. If NONE of the four mechanisms has the name and `!if_exists`,
   return the existing "index not found" error unchanged.

## What NOT to do

- Do NOT touch F-50 Step 1/2/3a's landed generation-gate mechanism or the
  new `IndexState`/`state.rs` (unrelated — those are about CREATE-time
  races, this is about DROP not existing at all).
- Do NOT design or implement crash-safety for a drop interrupted mid-way
  (e.g. `drop_all()` succeeds but the registry removal or final persist
  doesn't) — that composes with F-50 Step 3b's crash/restart work
  (tracked separately, #873) and is out of scope here. A best-effort,
  non-atomic sequence (matching how `create_index_v2`'s own multi-step
  sequence is already non-atomic pre-Step-3b) is acceptable for this task.
- Do NOT add index2/sorted support to `handle_create_index`'s `if_exists`
  dup-guard beyond what's already there — that path already dispatches
  correctly by `op.index_type` (`:374`); this brief is DROP-side only.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Write tests covering: dropping an index2 backend (functional or fts,
  your choice) by name via the DDL path and confirming (a) it's gone from
  `index2_registry`, (b) a table reopen (persist round-trip) does NOT
  resurrect it, (c) its postings are actually cleaned up (not just
  registry-removed) if `drop_all()`'s contract supports checking that;
  dropping a sorted index by name via the DDL path with the same
  round-trip check; the `if_exists` early-exit correctly reporting
  `existed: true` for an index2/sorted index that exists (not just
  legacy ones); dropping a non-existent name with `if_exists: false`
  still returns the existing "not found" error unchanged (regression
  guard for the common case).
- Clean up any scratch/debug log files you create in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-engine --full
```

When done, give your final summary as plain text: the exact new/changed
functions and their signatures, the resolution-order logic you wired into
`handle_drop_index`, your test results (actual output) for each of the
four scenarios listed above, and confirmation fmt/clippy are clean.
