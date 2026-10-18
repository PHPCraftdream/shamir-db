# 02h — regroup base_index/tests tree into topical subfolders + split basic/unique test files

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

## MOVE PLAN (chunk C1): under `crates/shamir-index/src/base_index/tests/`.

SPLITS (test fn names stay identical; module paths may change; each created file ≤950 lines):
1. `index_manager_tests/basic_tests.rs` (1240 lines) → folder `index_manager_tests/basic/`: `mod.rs` (manifest: `mod` decls + `pub use` of any shared helper items the split parts need, marked appropriately) + 2 files split by test topic. The file's `use super::helpers::{create_manager, create_nested_value, create_test_value};` must become `use crate::base_index::tests::index_manager_tests::helpers::{…};` in each part (helpers.rs STAYS at `index_manager_tests/helpers.rs`). Any intra-file shared setup fns/structs live in ONE part with `pub(super)` visibility (or in the mod.rs-adjacent part) — never duplicated.
2. `index_manager_tests/unique_tests.rs` (1263 lines) → folder `index_manager_tests/unique/`: same recipe, 2 files, same helpers-import fix (`use super::helpers::…` → crate-absolute).

REGROUPS (pure moves; fix `use super::helpers::…` → `use crate::base_index::tests::index_manager_tests::helpers::…` in every moved file that has it; some files may also have inline `super::helpers::…` calls — fix those the same way):
3. `index_manager_tests/concurrency/` ← `audit_a9_concurrency_tests.rs`, `concurrency_tests.rs`, `p1058_in_flight_build_registry_tests.rs`, `p1098_writer_order_tests.rs`, `p1102_regular_index_gen_order_tests.rs`
4. `index_manager_tests/data/` ← `btree_tests.rs`, `byte_identity_tests.rs`, `composite_tests.rs`, `type_tests.rs`
5. `index_manager_tests/persistence/` ← `f72_publish_then_persist_tests.rs`, `f78_streaming_equivalence_tests.rs`
6. `sorted_index_manager_tests/generation/` ← `f71_epoch_init_tests.rs`, `p1007_rename_generation_tests.rs` — these use `super::helpers::{fresh_mgr, record_with_int, enc_i64}` pointing at `sorted_index_manager_tests/helpers.rs` which STAYS put; rewrite to `use crate::base_index::tests::sorted_index_manager_tests::helpers::…;` and fix the inline `super::helpers::fresh_mgr()` call sites (×5 in f71) to the crate-absolute path.
7. `info/` ← `index_definition_tests.rs`, `index_info_item_tests.rs`, `index_info_tests.rs`, `index_record_key_tests.rs`, `index_status_tests.rs`
8. `durability/` ← `migration_freeze_tests.rs`, `p03_drop_durability_tests.rs`, `p03b_sorted_drop_durability_tests.rs`, `p05b_sorted_rename_durability_tests.rs`
9. `ddl/` ← `f72_legacy_state_compat_tests.rs`, `p1068_ddl_op_log_retention_tests.rs`, `p12_ddl_partial_error_tests.rs`, `write_barrier_flags_tests.rs`

Each new subfolder gets a `mod.rs` manifest (one `pub mod` decl per member, nothing else). Update `index_manager_tests/mod.rs` (→ decls: basic, concurrency, data, helpers, persistence, unique), `sorted_index_manager_tests/mod.rs` (→ basic_tests, byte_identity_tests, covering_tests, generation, helpers, tx_plan_tests), and `base_index/tests/mod.rs` (→ ddl, durability, index_manager_tests, info, sorted_index_manager_tests). The moved test files themselves are NOT edited beyond their helper-import lines. Final counts: `base_index/tests/` = 6 entries; `index_manager_tests/` = 7; `sorted_index_manager_tests/` = 7; every new subfolder ≤6.
