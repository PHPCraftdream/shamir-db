You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
ddl-poison-1895). Read the root CLAUDE.md (binding; read the section "Sanctioned std::sync::Mutex exceptions" in
full) and `tools/rust-guard/README.md`. The brief below is the task; it is committed at
docs/dev-artifacts/prompts/index/01-ddl-guard-poison.md.

# Brief: a panic under a DDL guard mutex must not cascade (DDL-GUARD-POISON)

## Why

`crates/index/shamir-index/src/base_index/index_manager/flags.rs` (17 sites) and `.../sorted_index_manager/ddl.rs`
(10 sites), plus `create.rs` and `lookup.rs`, take `std::sync::Mutex` guards with `.lock().unwrap()`
(`dropping_regular`, `dropping_unique`, `renaming_regular`, `renaming_unique`, `dirty_sets`, `dropping_sorted`,
`renaming_sorted`). A `std::sync::Mutex` is poisoned when a thread panics while holding it; after that EVERY later
DDL or lookup on that index manager panics at the `unwrap`, so one failure becomes permanent unavailability of the
table's indexes. The project sanctions these mutexes as rare DDL guard sets, but says nothing about poisoning, and
there is precedent for moving off them for exactly this reason (the per-tx `ri_barrier_tokens` and `PredicateSet`
became lock-free `scc` structures: findings F-66/F-79).

## What (behaviour of the non-failing path must not change)

1. For EACH site decide, from the code, whether the guarded data can be left half-updated by a panic: these guards
   guard small sets/maps of index names updated by single insert/remove calls. Where a partial update is
   impossible (a single `insert`/`remove`/`contains` under the lock), recover from poisoning with
   `lock().unwrap_or_else(std::sync::PoisonError::into_inner)` and say so in a one-line comment naming the reason
   (not a generic one). Where a partial update IS possible (multi-step mutation), say so and either restructure so
   the invariant holds at every await/panic point or move that structure to the lock-free primitive the project
   standard prescribes (`scc::HashSet`/`scc::HashMap` with `THasher`; read the neighbouring code for the pattern).
   Prefer one helper function per manager that returns the guard with the recovery, so the policy lives in one
   place rather than 27 copies.
2. Tests (plain tests in the crate's `tests/` folders, multi-line literals): a thread panics while holding the guard
   (use the crate's test seam or a helper that locks and panics via `std::panic::catch_unwind` in a spawned thread),
   then a following drop-index / rename-index / lookup on the same manager succeeds and observes the state the
   panicking thread left (documented per site). One test per guard family at least.
3. Update the "Sanctioned std::sync::Mutex exceptions" paragraph of the root CLAUDE.md with one sentence stating that
   these guards recover from poisoning (and where).

## Scope and siblings

Only `crates/index/shamir-index` (and the one CLAUDE.md sentence). Other agents work on the layers gate, file splits
and the engine layering: do not touch those.

## Observable acceptance

`cargo fmt -p shamir-index` and `cargo clippy -p shamir-index --all-targets -- -D warnings` clean; clippy for the
dependants `-p shamir-engine -p shamir-db` if a signature changed (it should not); layout passes. CI observes the
full matrix. Non-goals: any behaviour change on the non-panicking path, other mutexes of the workspace (list any
you noticed with the same pattern in the final report instead of changing them).

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
