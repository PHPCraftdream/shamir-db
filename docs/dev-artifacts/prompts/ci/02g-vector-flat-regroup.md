# 02g — regroup vector/ flat files into quant/

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

## MOVE PLAN (chunk B3): inside `crates/shamir-index/src/vector/` (currently 11 direct entries; target ≤7).

1. Create `crates/shamir-index/src/vector/quant/` with `mod.rs` (manifest: one `pub mod` decl per moved file + a short module doc: "Quantization primitives (SQ8, quantized distance, quant metadata) and the flat/brute-force adapter stack they serve.") and MOVE into it:
   - `adapter.rs`, `brute_force.rs`, `quant_meta.rs`, `quantized_dist.rs`, `sq8.rs`
2. Rewrite `crates/shamir-index/src/vector/mod.rs` so every old module path keeps resolving. Resulting `pub mod` decls: `quant`, `hnsw_adapter`, `simd`, `snapshot`, `vector_backend`, plus `#[cfg(test)] mod tests;` and the existing `pub use adapter::{SearchOpts, VectorAdapter};` / `pub use vector_backend::VectorBackend;` item re-exports re-pointed to their new locations (`pub use quant::adapter::{SearchOpts, VectorAdapter};`, `pub use vector_backend::VectorBackend;` — note the `vector_backend` MODULE decl is unchanged, only the item re-export source path may need updating). Add module path re-exports: `pub use quant::adapter; pub use quant::brute_force; pub use quant::quant_meta; pub use quant::quantized_dist; pub use quant::sq8;`
3. Fix `super::` imports inside the 5 moved files (measured: `adapter.rs` ×1, `brute_force.rs` ×2, `quant_meta.rs` ×1, `quantized_dist.rs` ×4) → absolute `crate::vector::…` paths (they keep resolving via the re-exports above).
Final count of `vector/`: mod.rs, tests/, hnsw_adapter/, simd/, snapshot/, vector_backend/, quant/ = 7 entries. (The `hnsw_adapter/`, `simd/`, `snapshot/`, `vector_backend/` folders are created by other workers — your `mod.rs` decls for them already exist today as `pub mod …;` and stay.)
