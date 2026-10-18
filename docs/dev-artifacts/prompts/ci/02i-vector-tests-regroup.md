# 02i — regroup vector/tests tree into topical subfolders + split 3 oversized test files

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

## MOVE PLAN (chunk D): under `crates/shamir-index/src/vector/tests/`.

SPLITS (test fn names stay identical; each created file ≤950 lines; intra-file shared helper structs/fns live in ONE part with `pub(super)` or in a `support.rs` inside the same folder — never duplicated; these test files import via absolute `crate::…` paths, which keep resolving):
1. `compaction_tests.rs` (1107 lines) → folder `lifecycle/compaction_tests/`: `mod.rs` + 2 files split by test topic.
2. `hnsw_adapter_tests.rs` (1327 lines) → folder `hnsw/hnsw_adapter_tests/`: `mod.rs` + 2 files.
3. `quantized_graph_tests.rs` (2095 lines) → folder `quant/quantized_graph_tests/`: `mod.rs` + 3 files. Its import `use crate::vector::hnsw_adapter::{install_test_search_f32_gate, HnswAdapter, HnswConfig};` stays exactly as-is (the re-exports on the hnsw_adapter folder keep it alive). The RAII guard struct around line 1885 (`…GateGuard`) and any other shared helpers move to one part with `pub(super)` visibility.

REGROUPS (pure moves; subfolder `mod.rs` = one `pub mod` decl per member):
4. `hnsw/` ← `hnsw_adapter_tests/` (the split folder), `hnsw_rs_contract_tests.rs`, `deadlock_regression_tests.rs`
5. `quant/` ← `quantized_graph_tests/` (the split folder), `quantization_snapshot_tests.rs`, `quantized_dist_tests.rs`, `sq8_tests.rs`
6. `backend/` ← `brute_force_tests.rs`, `simd_tests.rs`, `vector_backend_tests.rs`, `vector_restore_tests.rs`
7. `lifecycle/` ← `compaction_tests/` (the split folder), `cofilter_prefilter_tests.rs`, `crash_recovery_tests.rs`, `delta_log_tests.rs`, `snapshot_tests.rs`

Update `vector/tests/mod.rs` to: `pub mod backend; pub mod hnsw; pub mod lifecycle; pub mod quant;`. Final count of `vector/tests/`: mod.rs + 4 subfolders = 5. Folder counts: hnsw/ = 4, quant/ = 5, backend/ = 5, lifecycle/ = 6. The moved (unsplit) test files are NOT edited.
