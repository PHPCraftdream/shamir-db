# CI-02 layout sweep — crate shamir-storage

## Compile / build / test policy — MANDATORY, highest priority (binds you and any child)
**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t`/`cargo tl`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build. Allowed locally, scoped to touched crates: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`. One cargo process at a time, CARGO_BUILD_JOBS=4. Never silence a lint with `#[allow]`. Fix every compiler/clippy message these commands print; do not leave them for CI.

## Git rule
⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (including `git add` / `git commit` / `git mv`). Only edit files; the orchestrator commits. Moving a file = create the new file with your file tool and delete the old one with your file tool.

## Goal
`scripts/ci/check_layout.py` enforces ≤7 direct entries per folder and ≤1000 physical lines per source file; violations are grandfathered in `scripts/ci/layout_baseline.txt` (a ratchet — nothing may grow). This sweep resolves, for crate shamir-storage ONLY (crates/shamir-storage/), ALL of:
- dir crates/shamir-storage/src — 13 entries → ≤7
- dir crates/shamir-storage/src/key_bytes/tests — 8 entries → ≤7
- file crates/shamir-storage/src/storage_membuffer.rs — 1156 lines → split (every resulting file ≤1000)
- file crates/shamir-storage/src/tests/stores/storage_cached_tests.rs — 1212 lines → split
- file crates/shamir-storage/src/tests/stores/storage_membuffer_tests.rs — 1075 lines → split
- file crates/shamir-storage/src/tests/stores/storage_mirrored_tests.rs — 1364 lines → split
PURE STRUCTURAL REFACTOR: behaviour, public API paths, error behaviour, test semantics and test function names stay identical.

## Current src/ entries (13)
README.md, error.rs, key_bytes.rs, key_bytes/ (contains only tests/), lib.rs, storage_cached.rs, storage_fjall.rs, storage_in_memory.rs, storage_membuffer.rs, storage_mirrored.rs, test_seams/, tests/, types.rs

## Target layout
src/ (7 entries): README.md, lib.rs, core/, key_bytes/, stores/, test_seams/, tests/
- `core/` (3): mod.rs (manifest only), error.rs, types.rs — moved unchanged.
- `key_bytes/`: merge the pair key_bytes.rs + key_bytes/tests/ into key_bytes/{mod.rs, core.rs, tests/}. key_bytes.rs content → key_bytes/core.rs; NEW key_bytes/mod.rs = manifest only (`mod core;` + re-export matching old visibility + `#[cfg(test)] mod tests;`). Old crate paths (`crate::key_bytes`, external `shamir_storage::key_bytes::...`) must resolve identically — lib.rs declares `mod key_bytes;` exactly as before, nothing else changes there.
- `key_bytes/tests/` (8 entries) → regroup into ≤7: create topic subfolders, e.g. `tests/compare/` (eq_ord_tests.rs, hash_consistency_tests.rs) and `tests/serde/` (serde_byte_identity_tests.rs) etc. — each new subfolder gets its own mod.rs (manifest only) and the parent key_bytes/tests/mod.rs lists the subfolders instead of the moved files. Test function names unchanged.
- `stores/` (≤7): mod.rs (manifest only) + the five storage backends:
  - storage_cached.rs, storage_fjall.rs, storage_in_memory.rs, storage_mirrored.rs move as-is (each becomes a module file in stores/); storage_mirrored.rs currently 1364... wait, that's its TEST file; the impl file is fine unless >1000 — check `wc -l` and split on seams only if >1000.
  - storage_membuffer.rs (1156 lines) → stores/storage_membuffer/ directory module: `mod.rs` (manifest only, re-exports), plus 2-3 files split along natural seams (e.g. `core.rs` + `flush.rs` or by impl-block groups), each ≤1000 lines, "one file = one primary export"; impl blocks of the same type may live in several files of the same module. Module path `crate::storage_membuffer` must keep resolving: name the dir `storage_membuffer/` (directory module keeps the identical module path — preferred) instead of `membuffer/`.
  - So stores/ = mod.rs, storage_cached.rs, storage_fjall.rs, storage_in_memory.rs, storage_mirrored.rs, storage_membuffer/ = 6 entries ✓
  - lib.rs keeps declaring `mod storage_cached;` etc. exactly as before — the moves are under stores/, so lib.rs declares `mod stores;` and then re-exports/declares each backend at its old path matching ORIGINAL visibility (read lib.rs and mirror it exactly: if it was `pub mod storage_cached;` keep an equivalent `pub use stores::storage_cached;`... choose the form that keeps every old path valid with zero external edits; search workspace with rg for `shamir_storage::` to confirm nothing else needs changes).
- Splitting the three oversized TEST files: convert each file to a same-named directory module so module paths and test names stay identical, e.g. `tests/stores/storage_cached_tests.rs` → `tests/stores/storage_cached_tests/mod.rs` (manifest only: `mod <topic>;` lines) + topic files (`basic_tests.rs`, `iteration_tests.rs`, ...) split along topical seams, each ≤1000 lines, every `#[tokio::test]`/`#[test]` function moved verbatim (names unchanged). Same for storage_membuffer_tests and storage_mirrored_tests. Update tests/stores/mod.rs only if the module declarations change form (directory module keeps `mod storage_cached_tests;` — usually unchanged).

## Rules
- `mod.rs` files: re-exports/`mod` declarations only.
- Every moved file declared by exactly one `mod` (no orphans, no double mounts).
- `super::`/`crate::` paths re-checked after every move; a split impl loses access to private fields — keep split impl files inside the same module (storage_membuffer/), not siblings.
- Imports at the top of each file; remove unused imports; test-only imports `#[cfg(test)]`.
- No new intra-doc links to private items; plain backticks fine.
- Do not grow any file already over 1000; do not touch unrelated code.

## Gates you must run and make clean (real exit codes in your report)
`cargo check -p shamir-storage --all-targets`, `cargo clippy -p shamir-storage --all-targets -- -D warnings`, `cargo fmt -p shamir-storage`, `python -I scripts/ci/check_layout.py`.
Then hand-verify (the checker does NOT see untracked new files): `ls -A` every folder you created or changed — each ≤7 entries; `wc -l` every new/changed file — each ≤1000 lines.

## Final report
Before/after entry count of every folder touched, line counts of every new/moved/changed file, real exit codes of the four gate commands, list of every file moved/created/deleted, any deviation with reason.
