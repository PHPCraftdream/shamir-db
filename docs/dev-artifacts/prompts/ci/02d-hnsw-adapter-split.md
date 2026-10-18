# 02d — split vector/hnsw_adapter.rs (2984 lines) into a folder module

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

## MOVE PLAN (chunk B1)

`crates/shamir-index/src/vector/hnsw_adapter.rs` → folder `crates/shamir-index/src/vector/hnsw_adapter/` (module path `crate::vector::hnsw_adapter` unchanged; the `pub mod hnsw_adapter;` decl in `vector/mod.rs` stays — do NOT edit `vector/mod.rs`).

Files (≤7 entries; suggested seams — adjust as content demands, never split inside a fn; the `impl VectorAdapter for HnswAdapter` trait-impl block must stay WHOLE in one file):
- `mod.rs` — manifest: module docs, `mod` decls, `pub use` re-exports of every module-level `pub`/`pub(crate)` item: `MAX_TOPK`, `MAX_EF_SEARCH`, `ShamirDist` (if pub), `HnswConfig`, `HnswAdapter`, `CO_FILTER_EF_MULTIPLIER`, `PRE_FILTER_MAX_CANDIDATES`, `CO_FILTER_MAX_SELECTIVITY`, AND the `#[cfg(test)]` items `TestF32Gate`, `install_test_search_f32_gate`, and the gate-clearing fn — each re-exported under `#[cfg(test)]`. Known external consumer that must keep compiling: `vector/tests/quantized_graph_tests.rs` does `use crate::vector::hnsw_adapter::{install_test_search_f32_gate, HnswAdapter, HnswConfig};`.
- `config.rs` — `MAX_TOPK`, `MAX_EF_SEARCH`, `BRUTE_FORCE_MAX`, `FIT_THRESHOLD`, `CATCHUP_SPIN_BUDGET`, `CATCHUP_BACKOFF`, `ShamirDist` + its `Distance<f32>` impl, `HnswConfig` + `Default` (~lines 1–180).
- `adapter.rs` — `struct HnswAdapter` (all fields + docs), `new`, `new_with_quantization`, fit/catchup/publish machinery, and the quantize helpers `quantize`, `claim_and_publish_u8`, `bump_migrated_on_tombstone`, `quantized_active` (~354–1230).
- `search.rs` — the `#[cfg(test)]` test gate items (`TestF32Gate`, `TEST_SEARCH_F32_GATE` static, `install_test_search_f32_gate`, clear fn) plus the main search/collect paths (~1230–1975). The static is read from two sibling files → mark it `pub(super)`.
- `cofilter.rs` — `CO_FILTER_EF_MULTIPLIER`, `PRE_FILTER_MAX_CANDIDATES`, `CO_FILTER_MAX_SELECTIVITY` + the co-filter/prefilter impl block (~1976–2283), including its inner `#[cfg(test)]` bit.
- `vector_impl.rs` — the whole `impl VectorAdapter for HnswAdapter` block (~2283–end) including its `#[cfg(test)]` bits.
Keep `#[cfg(test)]`-gated methods on their methods. `pub(super)` for struct fields / private fns that siblings use. Balance ≤950 lines each.
