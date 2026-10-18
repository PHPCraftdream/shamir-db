# 02j — regroup src/ top-level files into backends/, fts/, model/ + lib.rs wiring

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

## MOVE PLAN (chunk E): `crates/shamir-index/src/` currently has 23 direct entries; target ≤7. Keep at root: `lib.rs`, `base_index/`, `vector/`, `tests/`. Move the 19 loose files into three new group folders (create each subfolder's `mod.rs` as a manifest of `pub mod` decls + a one-line module doc):

1. `backends/` ← `backend.rs`, `build_backend.rs`, `fts_backend.rs`, `fts_ranked_backend.rs`, `functional_backend.rs` (folder = 6 entries)
2. `fts/` ← `bm25.rs`, `tokenizer.rs` (folder = 3 entries)
3. `model/` ← `descriptor.rs`, `kind.rs` at its root; plus subfolders:
   - `model/runtime/` ← `actor.rs`, `lifecycle.rs`, `reader_drain_gate.rs`, `registry.rs`, `state.rs`, `write_ops.rs` (7 entries)
   - `model/meta/` ← `meta_envelope.rs`, `persistence.rs` (3 entries)
   - `model/query/` ← `expr.rs`, `posting_layout.rs` (3 entries)
   (`model/` = mod.rs + descriptor.rs + kind.rs + runtime/ + meta/ + query/ = 6 entries)
   IMPORTANT: do NOT use the name `core` for any folder or file — it would shadow the `core` crate.
4. Rewrite the `crates/shamir-index/src/lib.rs` module declarations: remove the 19 `pub mod <name>;` decls for moved files, add `pub mod backends; pub mod fts; pub mod model;` (keep `pub mod base_index;`, `pub mod vector;`, `#[cfg(test)] mod tests;` and the file's doc comments untouched), then add module path re-exports so EVERY old module path keeps resolving (the moved files and all other crate/cross-crate code import via `crate::<name>`):
```rust
pub use backends::backend;
pub use backends::build_backend;
pub use backends::fts_backend;
pub use backends::fts_ranked_backend;
pub use backends::functional_backend;
pub use fts::bm25;
pub use fts::tokenizer;
pub use model::descriptor;
pub use model::kind;
pub use model::meta::meta_envelope;
pub use model::meta::persistence;
pub use model::query::expr;
pub use model::query::posting_layout;
pub use model::runtime::actor;
pub use model::runtime::lifecycle;
pub use model::runtime::reader_drain_gate;
pub use model::runtime::registry;
pub use model::runtime::state;
pub use model::runtime::write_ops;
```
5. Update lib.rs's existing ITEM re-exports (the `pub use actor::IndexActor;` … block) to point at the new module locations, e.g. `pub use model::runtime::actor::IndexActor;`, `pub use backends::backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};`, `pub use model::query::expr::{ExprError, IndexExpr};`, `pub use model::kind::{…}`, `pub use model::meta::meta_envelope::{MetaEnvelope, MetaError, ENVELOPE_MAGIC, ENVELOPE_VERSION};`, `pub use model::meta::persistence::{…}`, `pub use model::query::posting_layout::{build_posting_key, type_tag, PostingKeyRef};`, `pub use model::runtime::reader_drain_gate::{DropDrainGuard, ReadGuard, ReaderDrainGate};`, `pub use model::runtime::registry::{BackendLease, IndexRegistry};`, `pub use model::runtime::state::IndexState;`, `pub use model::runtime::write_ops::{apply_index_ops, IndexWriteOp};`, `pub use backends::build_backend::{build_index2_backend, build_index2_backend_with_resolver};`, `pub use model::descriptor::IndexDescriptor;`. Every re-export that exists today must still exist.
Final count of `src/`: lib.rs, base_index/, vector/, tests/, backends/, fts/, model/ = 7 entries.

Note: none of the 19 files use `super::` imports (verified), and their absolute `crate::…` imports keep resolving via the lib.rs re-exports — the moved files themselves need NO import edits. Do not touch anything under `base_index/` or `vector/`.
