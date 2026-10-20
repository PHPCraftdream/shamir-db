# CI-02 layout sweep — crate shamir-sdk

## Compile / build / test policy — MANDATORY, highest priority (binds you and any child)
**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t`/`cargo tl`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build. Allowed locally, scoped to touched crates: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`. One cargo process at a time, CARGO_BUILD_JOBS=4. Never silence a lint with `#[allow]`. Fix every compiler/clippy message these commands print; do not leave them for CI.

## Git rule
⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (including `git add` / `git commit` / `git mv`). Only edit files; the orchestrator commits. Moving a file = create the new file with your file tool and delete the old one with your file tool.

## Goal
`scripts/ci/check_layout.py` enforces ≤7 direct entries per folder and ≤1000 physical lines per source file; violations are grandfathered in `scripts/ci/layout_baseline.txt` (a ratchet — nothing may grow). This sweep resolves, for crate shamir-sdk ONLY (crates/shamir-sdk/):
- dir crates/shamir-sdk/src has 12 direct entries → must go to ≤7
No file in this crate exceeds 1000 lines. PURE STRUCTURAL REFACTOR: behaviour, public API paths, error behaviour, test semantics and test names stay identical.

## Current src/ entries (12)
`__rt.rs context.rs db.rs error.rs host_imports.rs http.rs lib.rs params.rs prelude.rs tests/ validation.rs value.rs`

## Target layout for src/ (≤7 entries)
- `lib.rs`, `error.rs`, `prelude.rs`, `tests/` — stay put (prelude may need its re-export list updated to point through the new group modules — keep every old public path resolving).
- `api/` — new folder with `mod.rs` (manifest, re-exports only): move `context.rs`, `db.rs`, `http.rs`, `value.rs` into it.
- `runtime/` — new folder with `mod.rs` (manifest, re-exports only): move `__rt.rs`, `host_imports.rs`, `params.rs`, `validation.rs` into it.
Adjust the split only if the real module graph demands it (privacy or cfg-gating). Every old public path must still resolve: if lib.rs declared `pub mod context;` and callers use `shamir_sdk::context::X`, keep it working either by declaring inside api/mod.rs `pub mod context;` + re-exporting `pub use context::*;` and in lib.rs `pub use api::context;` — pick whichever keeps ALL old paths valid with zero edits outside the crate. Zero changes expected in other crates; if you find any external reference to a moved path (search the workspace with rg for `shamir_sdk::`), keep that path valid.

## Rules
- `mod.rs` files contain re-exports/`mod` declarations only.
- Every moved file declared by exactly one `mod` (no orphans, no double mounts).
- `super::` paths inside moved files that pointed at lib.rs items now resolve to the group mod.rs — re-check each; route via `crate::` or re-exports.
- Imports at the top of each file; remove unused imports after the move. Test-only imports need `#[cfg(test)]`.
- No new intra-doc links to private items; plain backticks are fine.
- Do not touch code unrelated to the move.

## Gates you must run and make clean (real exit codes in your report)
`cargo check -p shamir-sdk --all-targets`, `cargo clippy -p shamir-sdk --all-targets -- -D warnings`, `cargo fmt -p shamir-sdk`, `python -I scripts/ci/check_layout.py`.
Then hand-verify (the checker does NOT see untracked new files): `ls -A` every folder you created or changed — each ≤7 entries; `wc -l` every new/changed file — each ≤1000 lines.

## Final report
Before/after entry count of every folder touched, line counts of every new/moved/changed file, real exit codes of the four gate commands, list of every file moved/created/deleted, any deviation with reason.
