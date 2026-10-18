# 02b — split base_index/index_manager.rs (3020 lines) and index_manager_unique.rs (1149 lines) into folder modules

Pure structural refactor of `crates/shamir-index/` (CI-02 layout sweep). Behaviour, public API paths, error behaviour, test semantics and test function names stay identical. Repo root is your working directory; all paths below are repo-root-relative.

## Hard rules (violating any = rejected)
- ⛔ NEVER run `git reset` / `git checkout` / `git clean` / `git stash` / `git restore` / `git rm` / `git mv`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. Read-only git (`git status`, `git diff`, `git show`) is fine.
- Moving a file = create the new file with your file tool (copy the content), then delete the old file with plain `rm <path>` (POSIX file delete, NOT `git rm`). Never `git mv`.
- NO compile/test commands, ever: no `cargo test` / `build` / `check` / `clippy` / `nextest` / `fmt` / `npm` / `tsc` / `rustc`. Allowed: `python -I scripts/ci/check_layout.py` (instant, read-only, no build) and plain file operations.
- Every `.rs` file you leave behind must be ≤950 lines by `wc -l` (hard limit 1000; the orchestrator runs `cargo fmt -p shamir-index` afterwards). Any folder you touch or create must have ≤7 direct entries (files + subdirs, `mod.rs` counts).
- Never silence a lint with `#[allow]`. No new dependencies. No machine-specific absolute paths in any file.
- `use` statements at the top of each file. `mod.rs` files contain module declarations + `pub use` re-exports ONLY (plus doc comments) — no logic.
- Do not modify anything outside the paths this brief lists. Do not reformat unchanged code; preserve doc comments, attribute placement and item order except where the move requires adjustment.
- Pure structural refactor: no behaviour, signature, item-name, or test-name changes.

## Path-stability contract
Every module path and public item path that exists today (`shamir_index::…`, `crate::…`) must keep resolving after your change. For a file that becomes a folder: the folder's `mod.rs` re-exports every item that was `pub`/`pub(crate)` at the old module level (`pub use inner::Item;`), and any `#[cfg(test)]`-only item's re-export gets the same `#[cfg(test)]` attribute. Private/free-fn helpers used across the new sibling files get `pub(super)` (or, where this brief says, `pub(in crate::base_index)`). Before splitting a file, enumerate its module-level items (`rg -n "^(pub |pub\(crate\) |pub\(super\) )?(struct|enum|trait|impl|fn|const|static|type|mod|use)" <file>` plus `#\[cfg\(test\)\]` lines) so nothing is dropped or duplicated.

## Verify before reporting
1. `python -I scripts/ci/check_layout.py` — include the full output in your report; no NEW violations may appear at paths you touched (pre-existing baselined entries outside your scope are expected and fine).
2. `git status --porcelain` — only paths this brief lists may appear; include it in your report.
3. Re-read every file you created or edited and confirm: no item lost, no item duplicated, every `mod` declaration matches a real file, every `use` path resolves to a real item, every formerly-public item is re-exported.
4. Report: created/deleted/edited file list WITH final line counts of every created file, the checker output, and anything you had to deviate from.

## MOVE PLAN (chunk A1)

1. `crates/shamir-index/src/base_index/index_manager.rs` → folder `crates/shamir-index/src/base_index/index_manager/` (module path `crate::base_index::index_manager` is unchanged; the existing `pub mod index_manager;` declaration in `base_index/mod.rs` stays as-is — do NOT edit `base_index/mod.rs`).

   Files (≤7 entries total in the folder; suggested seams — adjust as content demands, but NEVER split inside a function body; an inherent `impl` block may be split across files by moving whole methods into multiple `impl IndexManager` blocks):
   - `mod.rs` — manifest: keep the original file's module-level doc comments here, one `mod` decl per sibling, and `pub use` re-exports for every item that was `pub`/`pub(crate)` at module level (structs `IndexManager`, `HashRenameTombstone`, `HashDropTombstone`, any pub consts/types, and any `#[cfg(test)]` items with matching gates).
   - `inner.rs` — header `use` block (the original file's imports, trimmed per-file later), file-level helper fns (e.g. `regular_provenance`), consts (e.g. `BACKFILL_PROGRESS_LOG_INTERVAL`, `POSTING_CACHE_CAP`), `HashRenameTombstone`, `HashDropTombstone`, the `struct IndexManager` definition with ALL its fields and their doc comments, `impl Clone for IndexManager`, and the constructor/init part of the inherent impl.
   - `hooks.rs` — the test/pause-hook setter methods (`set_create_index_backfill_hook`, `set_drop_index_pause_hook`, `set_lookup_pause_hook`, `reader_drain_waits`, `reader_gate`, `set_drop_index_post_sweep_hook`, `set_drop_index_status_pause_hook`, `set_rename_mid_pause_hook`, `set_recover_renames_between_entries_hook`, and the other `set_*` accessors).
   - `flags.rs` — `sync_flags`, `mark_build_in_flight`, `is_build_in_flight`, `clear_build_in_flight`, dirty-set machinery (`drain_dirty_set`, `get_or_create_dirty_set`), `has_indexes`, `has_unique_indexes`, `generation`, `write_barrier_flags`.
   - `create.rs` — create-index + backfill machinery (the ~lines 1000–2250 region that implements `create_index`/backfill).
   - `drop_rename.rs` — drop-index / rename-index / tombstone-sweep machinery (~lines 2250–2890).
   - `lookup.rs` + `listing.rs` — lookup/write-apply hot paths and posting-cache invalidation (`invalidate_posting_cache_for_ops`, `invalidate_posting_cache_for_keys`), then `iter_indexes`, `iter_indexes_ready`, `index_exists`, `get_index_definition` (~2889–end). If this leaves a file over 950 lines, rebalance methods between `create.rs` / `drop_rename.rs` / `lookup.rs` / `listing.rs`.

   CRITICAL visibility rule: today `IndexManager`'s fields are `pub(super)` — visible in the whole `base_index` subtree, and `crate::base_index::index_manager_unique` (a SIBLING module) accesses fields like `self.data_store`, `self.info_store`, `self.indexes_unique`, `self.write_barrier_flags`, and the drop-hook fields DIRECTLY (verified by grep). After the move the struct lives in `index_manager::inner`, where plain `pub(super)` would only cover the `index_manager` subtree and would BREAK `index_manager_unique`. Therefore change EVERY `pub(super)` field of `struct IndexManager` to `pub(in crate::base_index)` — this reproduces exactly the old visibility scope, no wider. Field doc comments and attributes stay byte-identical otherwise. Free fns/consts in `inner.rs` used by sibling files get `pub(super)`.
   Private (non-pub) methods that move into a different file than their callers also get `pub(super)`. `#[cfg(test)]`-gated methods keep their gate.

2. `crates/shamir-index/src/base_index/index_manager_unique.rs` → folder `crates/shamir-index/src/base_index/index_manager_unique/` (module path unchanged; do NOT edit `base_index/mod.rs`):
   - `mod.rs` — manifest + re-exports (incl. the `#[cfg(test)]` hook items with gates preserved).
   - `validate.rs` — `unique_provenance`, the `#[cfg(test)]` OnceLock hook global and its accessor fns, and the big `impl IndexManager` unique-validation/creation block (`create_unique_index_from_records`, `check_unique_constraint`, `check_unique_key`, `validate_unique_for_create_with_defs`, `plan_records_created_unique_batch`, `add_unique_entry_by_key`, `remove_unique_entry_by_key`, `apply_ops` unique handling, `save_index_info_unique`).
   - `listing.rs` — `unique_keys_for`, `iter_unique_indexes`, `unique_index_exists`, `get_unique_index_definition`.
   The moved code's import `use super::index_manager::IndexManager;` must become `use crate::base_index::index_manager::IndexManager;` (and similarly any other `super::` import — after the move `super` no longer means `base_index`). Items it references from `index_manager` (fields/methods) keep resolving via the field-visibility rule above and the `index_manager/mod.rs` re-exports; methods called across the two folders that were private must become `pub(super)`... NOTE: `pub(super)` from `index_manager/validate.rs`'s perspective is NOT enough to reach `index_manager_unique` — for any non-pub method of `IndexManager` that `index_manager_unique` calls, use `pub(in crate::base_index)` (same scope argument as the fields). Enumerate cross-folder calls with `rg -n "self\.[a-z_]+\(" crates/shamir-index/src/base_index/index_manager_unique/` and check each against the definition's visibility.
