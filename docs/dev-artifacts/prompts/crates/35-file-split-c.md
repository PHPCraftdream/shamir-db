You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on a branch named after
this task). Read the root CLAUDE.md (binding, especially "Layout limits", "Test organisation" and the rust-guard
gates) and `tools/rust-guard/README.md`. The brief below is the task; it is committed in this worktree under
docs/dev-artifacts/prompts/ (the file name is the task slug).

# Brief: split files that sit at the 1000-line limit (FILE-HEADROOM, group c)

## Why

The layout rule is at most 1000 lines per source file, and a number of files sit exactly at it (1000, 999, 998, 997,
993 lines). Every functional change in such a file forces a split inside that change or squeezes lines to fit (an
engine layering step already had to rewrite three call sites to get a file from 1001 back to 986). This task removes
that debt for your group by splitting each file along its natural seams into files of roughly 400 to 700 lines.

Your group's files are listed in the section "Files" below. Read each file completely first, find the seams
(distinct responsibilities, helper clusters, impl blocks of one type that group by concern, test-support code), then
split.

## What (behaviour must not change; public paths must keep resolving)

1. Pure moves: no logic change, no renames of public items, no signature change, no reordering of operations. Where a
   split needs an item to be visible to its sibling, use the narrowest visibility that works (`pub(super)` / the
   existing level); every widening is declared later through the campaign delta, so keep them few and list each one
   in the final report.
2. Repo rules: one file = one primary export (the repo's convention for new files; a closely coupled group is fine);
   `mod.rs` files are declarations and re-exports only (no logic); at most 7 entries per folder (turning `foo.rs`
   into a folder `foo/` with `mod.rs` + parts is the usual shape: keep the old path as `foo/mod.rs` re-exporting
   what the old file exported so every `use` elsewhere keeps working); no function-local `use` in any file you touch
   (hoist them); no new `#[allow]`; tests stay in `tests/` folders (declare `#[cfg(test)] mod tests;` where a split
   creates a module for them).
3. Public API of the crate is unchanged: run clippy for the crate AND for its dependants that import from it
   (`-p` list in the "Files" section).
4. rust-guard campaign: add `[[campaigns]]` to `.rust-guard/stages.toml` (`name = "file-split-c"`, `packages`
   = the crates of your files) and the two delta files `.rust-guard/stages/file-split-c.toml` and
   `file-split-c-discovery.toml`, each containing ONLY the line `base = "<sha of the origin/master tip you
   branched from>"`. The orchestrator fills the entries from the CI findings.

## Scope and siblings

Only the files of your group. Other agents split other groups in parallel; the engine layering waves work in
`shamir-engine` (`tx`, `repo`, `query/batch`, `table`): do not touch those files. If a test file of your crate must
be edited because a path moved, that is allowed.

## Observable acceptance

`cargo fmt -p <crates>` and `cargo clippy -p <crates> --all-targets -- -D warnings` clean (the `-p` list below);
layout passes (no file above ~800 lines among the ones you produced, no folder above 7 entries); public paths
unchanged; the final report lists every moved item and every widened visibility. CI observes the rust-guard gates,
rustdoc, engine-features (for engine crates), test-discovery (moved tests) and the full matrix. Non-goals: any
behaviour change, new features, renaming public API.

## Files (group c: wasm host and sync)

* `crates/wasm/shamir-wasm-host/src/wasm/wasm_function.rs` (1000 lines)
* `crates/server/shamir-sync/src/sync/replication/supervisor.rs` (1000 lines)
* `crates/server/shamir-sync/src/sync/replication/follower_loop.rs` (1000 lines)

Checks: `-p shamir-wasm-host -p shamir-sync -p shamir-server -p shamir-db`. The supervisor and follower loop are
long-running async state machines: keep every await point, select arm order and lock scope exactly as is; the wasm
function wrapper holds fuel and trap handling: move, do not touch.

## Lessons from the earlier waves (binding for this wave)

* **Do not hand-write the rust-guard delta entries.** Hand-derived deltas mismatched the gate in hundreds of places.
  Create the campaign entry in `.rust-guard/stages.toml` and the two delta files
  (`.rust-guard/stages/<name>.toml` and `<name>-discovery.toml`) containing ONLY the line
  `base = "<sha of the origin/master tip you branched from>"` (use `git rev-parse HEAD~0` of your worktree's base: the
  worktree was created from origin/master). The orchestrator fills the entries from the CI findings. Do record
  every item you moved, changed or added in your final report so the orchestrator can cross-check.
* **No function-local `use` in any file you touch** (the structure gate flags every local `use` in a changed file,
  also old ones): hoist them to the file header.
* **Code under `cfg(unix)`, `cfg(windows)` or `cfg(feature = ...)` is not compiled by your local clippy on this
  Windows machine.** Re-read every changed file for unused imports, wrong paths and privacy in such code
  (CI compiles it on Linux with all features).
* **Folder limit: at most 7 direct entries.** Counts today: `table/` 7, `table/support/` 7, `table/support/model/` 7,
  `table/table_manager/` 6, `repo/` 7, `repo/instance/` 7, `repo/instance/repo_instance/` 7, `tx/` 7, `query/batch/` 7,
  `table/support/storage/` 5, `table/support/guards/` 5, `db_instance/` 5. A new module must go to a folder with a
  free slot or you must regroup. `python -I scripts/ci/check_layout.py` is the authoritative check; if the sandbox
  refuses to run it, count entries and lines by hand and say so in the report. At most 1000 lines per file.
* A `mod.rs` that declares a new submodule is the file where the gate reports the new item; every new test module
  must be declared (`#[cfg(test)] mod tests;`) or its files are unreachable.
* Re-exports keep every public path working: `shamir_engine::{repo,table,tx,query,validator,meta,migration}::*` and
  everything `shamir-db`, `shamir-server*` and `shamir-sync` import must still resolve; clippy for
  `-p shamir-db` proves that.
* **No inherent `impl` of a lower-layer type in an upper layer** (for example `impl TableManager` inside
  `repo/instance`): it will not compile once the layers are separate crates. Use a free function or an extension
  trait in the upper layer instead.
* **Bundles expose methods, not public fields.** Do not make the shell reach into
  `self.<bundle>.<field>` for anything but construction; add a method on the bundle.
* **Do not duplicate a formula or a helper to avoid a dependency direction**: move the single implementation to the
  lower layer and call it from above.
* A file that would pass 1000 lines is split in the same change; do not compress lines to fit.

## Constraints (binding)

Compilation, builds and tests run in CI, run by the orchestrator, never on this machine. Banned: `cargo test`,
`cargo nextest`, `./scripts/test.sh`, `cargo build/run/bench/doc`, `npm`/`tsc`/`vitest`, wasm builds, e2e and
integration harnesses, loom, any release or full-workspace build, any command whose purpose is to build the project
or run its tests. Allowed locally, scoped to the crates you touched and nothing else:
`cargo fmt -p <crate>` (and `-- --check`), `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git (`status`, `diff`, `log`). Do not run the Python test suites either (CI runs them). Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --` with `-j 2`. A change you cannot verify here is verified by the orchestrator's CI
run: say in your final report exactly what must be observed there. If you believe you need a build or test run,
say so in the final report instead of running it.

Repo rules (read the root CLAUDE.md, it is binding): at most 7 entries per folder, at most 1000 lines per source
file, `mod.rs` re-exports only, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no visibility
widening, `shamir_collections::THasher` for hash maps, comments terse. Never write private machine paths, user
names, or the names of any CI infrastructure other than the workflow files into repository files. Never raise
versions. Do not touch code unrelated to the task.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Your last message must be a final report: what changed (files), what you verified locally (fmt/clippy/layout only),
what is unverified and must be observed in CI, anything you decided and why. Do not end on a tool call.
