# CI-02 layout sweep — crate shamir-funclib

## Compile / build / test policy — MANDATORY, highest priority (binds you and any child)
**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t`/`cargo tl`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build. Allowed locally, scoped to touched crates: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`. One cargo process at a time, CARGO_BUILD_JOBS=4. Never silence a lint with `#[allow]`. Fix every compiler/clippy message these commands print; do not leave them for CI.

## Git rule
⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (including `git add` / `git commit` / `git mv`). Only edit files; the orchestrator commits. Moving a file = create the new file with your file tool and delete the old one with your file tool.

## Goal
`scripts/ci/check_layout.py` enforces ≤7 direct entries per folder and ≤1000 physical lines per source file; violations are grandfathered in `scripts/ci/layout_baseline.txt` (a ratchet — nothing may grow). This sweep resolves, for crate shamir-funclib ONLY (crates/shamir-funclib/):
- dir crates/shamir-funclib/src has 32 direct entries → must go to ≤7
No file exceeds 1000 lines. PURE STRUCTURAL REFACTOR: behaviour, public API paths, error behaviour, test semantics and test names stay identical.

## Current src/ structure
17 function families, each as a pair `x.rs` (impl) + `x/` (containing only `x/tests/`): agg, arrays, canonical, cast, compare, crypto, datetime, gen, math, null, text, validate. Plus plain files: encode.rs, object.rs, strings.rs, registry.rs, scalar_resolver.rs, value_nav.rs, lib.rs, and `src/tests/` (6 entries, stays).

## Target layout
src/ (6 entries): lib.rs, registry.rs, scalar_resolver.rs, value_nav.rs, functions/, tests/
functions/ (6 entries): mod.rs, numeric/, value/, text/, crypto/, datetime/
- functions/numeric/ (5): mod.rs, agg/, arrays/, math/, gen/
- functions/value/ (7): mod.rs, cast/, compare/, null/, canonical/, encode.rs, object.rs
- functions/text/ (4): mod.rs, text/, strings.rs, validate/
- functions/crypto/ (3): mod.rs, core.rs, tests/   (crypto.rs → crypto/core.rs)
- functions/datetime/ (3): mod.rs, core.rs, tests/ (datetime.rs → datetime/core.rs)
For each family with an existing `x/tests/` dir: move `x.rs` to `x/core.rs` and create `x/mod.rs` as a manifest only: `mod core;` + `pub use core::*;` + `#[cfg(test)] mod tests;` (adjust to the exact previous wiring/visibility — if `x.rs` had non-pub items accessed via `x::item` inside the crate, `pub use core::*;` is fine internally; do not widen visibility beyond what the move needs). Families without a test dir (encode, object, strings) move as plain files into their group folder.
Group mod.rs files are manifests only (`mod`/`pub use`), no logic.

## Public path stability — the critical part
Read src/lib.rs first. It currently declares each family (`mod agg;` / `pub mod agg;` etc., possibly with cfg attributes) and possibly re-exports. Replace each old family declaration with a re-export of the moved module so that EVERY old path (`crate::agg`, `crate::agg::tests` wiring, external `shamir_funclib::...`) still resolves identically: e.g. where lib.rs had `mod agg;` use `pub(crate) use functions::numeric::agg;` (match original visibility exactly — do not widen). Preserve any `#[cfg(...)]` attributes that were on the old `mod` lines by putting them on the new `use` lines. registry.rs / scalar_resolver.rs / value_nav.rs stay at src/ root, unchanged declarations.
Test function names must not change; nextest module-path filters may change (acceptable). Every moved test module stays wired through its `x/tests/mod.rs` and the parent `#[cfg(test)] mod tests;`.

## Rules
- Every moved file declared by exactly one `mod` (no orphans, no double mounts).
- `super::` inside moved files/tests: a test dir that was sibling of `x.rs` and used `super::foo` now has `super` = functions/<grp>::x (via its mod.rs re-exports, usually still resolves — verify each).
- Imports at the top of each file; remove imports left unused after moves; test-only imports need `#[cfg(test)]`.
- No new intra-doc links to private items; plain backticks are fine.
- Do not touch code unrelated to the move.

## Gates you must run and make clean (real exit codes in your report)
`cargo check -p shamir-funclib --all-targets`, `cargo clippy -p shamir-funclib --all-targets -- -D warnings`, `cargo fmt -p shamir-funclib`, `python -I scripts/ci/check_layout.py`.
Then hand-verify (the checker does NOT see untracked new files): `ls -A` every folder you created or changed — each ≤7 entries; `wc -l` every new/changed file — each ≤1000 lines.

## Final report
Before/after entry count of every folder touched, line counts of every new/moved/changed file, real exit codes of the four gate commands, list of every file moved/created/deleted, any deviation from the target layout with reason.
