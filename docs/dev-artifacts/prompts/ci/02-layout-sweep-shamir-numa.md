# CI-02 layout sweep — crate shamir-numa

## Compile / build / test policy — MANDATORY, highest priority (binds you and any child)
**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t`/`cargo tl`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build. Allowed locally, scoped to touched crates: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`. One cargo process at a time, CARGO_BUILD_JOBS=4. Never silence a lint with `#[allow]`. Fix every compiler/clippy message these commands print; do not leave them for CI.

## Git rule
⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (including `git add` / `git commit` / `git mv`). Only edit files; the orchestrator commits. Moving a file = create the new file with your file tool and delete the old one with your file tool.

## Goal
`scripts/ci/check_layout.py` enforces ≤7 direct entries per folder and ≤1000 physical lines per source file; violations are grandfathered in `scripts/ci/layout_baseline.txt` (a ratchet — nothing may grow). This sweep resolves, for crate shamir-numa ONLY (crates/shamir-numa/):
- dir crates/shamir-numa/src has 12 direct entries → must go to ≤7
No file in this crate exceeds 1000 lines. This is a PURE STRUCTURAL REFACTOR: behaviour, public API paths, error behaviour, test semantics and test function names stay identical.

## Target layout for src/ (5 entries)
- `lib.rs`, `error.rs` — stay put.
- `core/` — new folder: move `cache_padded.rs`, `node.rs`, `node_replicated.rs`, `topology.rs` into it, plus a NEW `mod.rs` that is a manifest only (`mod cache_padded; pub use cache_padded::*;` style re-exports — no logic, no types). Verify first (read the files) that none of them has a sibling test subdir; if one does, carry the test dir along under core/ and wire it via core/mod.rs `#[cfg(test)] mod tests;` only if that's how it was wired before — preserve existing wiring exactly.
- `platform/` — new folder: move `cpulist.rs`, `detect.rs`, `fallback.rs`, `linux.rs`, `mock.rs` into it, plus NEW `mod.rs` manifest (re-exports only). Preserve any `#[cfg(target_os)]` gating on the moved `mod` declarations by putting the `#[cfg]` attributes on the `mod` lines inside platform/mod.rs.
- `tests/` — stays as is (7 entries, compliant).
Adjust the split only if the real module graph demands it (e.g. privacy: a moved module uses `pub(crate)` items fine, but if a moved file uses items private to the crate root that are NOT re-exported, keep that file where it is or re-export minimally). Public paths must not change: if `lib.rs` previously declared `mod node;` privately and re-exported, keep the same crate-external surface; if other crates reference `shamir_numa::node::...`, add `pub use` in lib.rs as needed so every old path still resolves. Zero changes expected in other crates.

## Rules
- `mod.rs` files contain re-exports/`mod` declarations only.
- Every moved file must be declared by exactly one `mod` (no orphans, no double mounts).
- `super::`/`crate::` paths inside moved files: files moved from src/ into src/<grp>/ keep the same crate — `crate::` paths unchanged; `super::` paths that pointed at lib.rs items now resolve to the group mod.rs — re-check each and route via `crate::` or re-exports as needed.
- Imports at the top of each file; remove imports left unused after the move.
- No new intra-doc links to private items; plain backticks are fine.
- Do not grow any file already over 1000 lines; do not touch code unrelated to the move.

## Gates you must run and make clean (real exit codes in your report)
`cargo check -p shamir-numa --all-targets`, `cargo clippy -p shamir-numa --all-targets -- -D warnings`, `cargo fmt -p shamir-numa`, `python -I scripts/ci/check_layout.py`.
Then hand-verify (the checker does NOT see untracked new files): `ls -A` every folder you created or changed — each ≤7 entries; `wc -l` every new/changed file — each ≤1000 lines.

## Final report
Before/after entry count of every folder you touched, line counts of every new/moved/changed file, real exit codes of the four gate commands, list of every file moved/created/deleted, and any deviation from the target layout with the reason.
