# 02c — split base_index/sorted_index_manager.rs (3023 lines) into a folder module

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

## MOVE PLAN (chunk A2)

`crates/shamir-index/src/base_index/sorted_index_manager.rs` → folder `crates/shamir-index/src/base_index/sorted_index_manager/` (module path `crate::base_index::sorted_index_manager` unchanged; do NOT edit `base_index/mod.rs`).

Files (≤7 entries; suggested seams — adjust as content demands, never split inside a fn):
- `mod.rs` — manifest: module docs, `mod` decls, and `pub use` re-exports of every module-level `pub`/`pub(crate)` item, INCLUDING the existing re-export `pub use crate::base_index::sorted_index_definition::SortedIndexDefinition;` which must survive verbatim (callers use `sorted_index_manager::SortedIndexDefinition`).
- `inner.rs` — header imports, `struct SortedIndexManager` (ALL fields + doc comments), `impl Clone`, constructor/init, hook setters (`set_drop_index_pause_hook`, `set_drop_index_post_sweep_hook`, `set_rename_rekey_pause_hook`), `reader_drain_waits`, `reader_gate`, `generation`, `last_mutation_version`, `note_mutation_at_version`, `has_indexes`, `has_covering_indexes`, `iter_indexes`, `find_by_field`, `find_by_field_ready`, `find_by_name_interned` (~lines 1–600).
- `ddl.rs` — register / create / drop / rename / sweep machinery (~600–1620).
- `plan.rs` — `intern_included_paths`, `plan_record_created`, `plan_records_created_batch`, `plan_record_updated`, `plan_record_deleted`, `bump_touched_indexes` (~1622–2000).
- `scan.rs` — the range-scan machinery (~2000–2807) including `range_bounds`, `entry_prefix`, `build_entry_key`, `has_indexable_value`.
- `covering.rs` — `type CoveringProjection` and the free helper fns (`build_covering_projection`, `inner_value_to_query_scalar`, `decode_covering_projection`, `extract_and_encode`, `key_value_slice`, `skip_boundary_row`, `decode_record_id_suffix`, `decode_sorted_index_name`) (~2808–end).

Visibility rule: `SortedIndexManager`'s fields are fully PRIVATE today (only this file touches them). After the split, sibling files inside the folder need them: change every field to `pub(super)` (scope = the `sorted_index_manager` module subtree — matches the split, no wider than needed). Private free fns and the `CoveringProjection` type used across sibling files get `pub(super)`. Private methods called across files get `pub(super)`. Balance file sizes ≤950 lines each.
