# 02f — regroup base_index/ flat files into meta/ and infra/

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

## MOVE PLAN (chunk A3): inside `crates/shamir-index/src/base_index/` (currently 15 direct entries; target ≤7).

1. Create `crates/shamir-index/src/base_index/meta/` with `mod.rs` (manifest: `pub mod` decls + nothing else) and MOVE into it (create new + `rm` old):
   - `index_definition.rs`, `sorted_index_definition.rs`, `index_info.rs`, `index_info_item.rs`, `index_status.rs`
2. Create `crates/shamir-index/src/base_index/infra/` with `mod.rs` and MOVE into it:
   - `backfill_pause_hook.rs`, `ddl_op_log.rs`, `index_keys.rs`, `index_record_key.rs`, `write_barrier_flags.rs`
3. Rewrite `crates/shamir-index/src/base_index/mod.rs`: keep the existing header doc comment and the `pub mod` decls for `backfill_pause_hook`-style paths via re-exports so EVERY old module path keeps resolving. Resulting shape (adjust doc comments, keep the existing explanatory comment about NOT being `#[cfg(test)]`-gated):

```rust
pub mod index_manager;
pub mod index_manager_unique;
pub mod sorted_index_manager;
pub mod infra;
pub mod meta;
#[cfg(test)]
pub mod tests;

pub use infra::backfill_pause_hook;
pub use infra::ddl_op_log;
pub use infra::index_keys;
pub use infra::index_record_key;
pub use infra::write_barrier_flags;
pub use meta::index_definition;
pub use meta::index_info;
pub use meta::index_info_item;
pub use meta::index_status;
pub use meta::sorted_index_definition;
```

4. Fix relative imports inside the 10 moved files: they contain `super::` imports (measured: `index_definition.rs` ×1, `index_info.rs` ×2, `sorted_index_definition.rs` ×2, `write_barrier_flags.rs` ×5) — after the move `super` no longer means `base_index`; rewrite them to absolute `crate::base_index::…` or `crate::…` paths (the re-exports above keep those paths alive).
Final count of `base_index/`: mod.rs, tests/, index_manager/, index_manager_unique/, sorted_index_manager/, infra/, meta/ = 7 entries.
