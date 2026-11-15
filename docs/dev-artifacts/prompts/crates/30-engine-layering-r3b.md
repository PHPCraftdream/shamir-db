You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
migration-down-1880). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and in full
`docs/dev-artifacts/design/engine-layering.md` (sections 5.15, 7.0, 7.3 item 4, Appendix A.2 and A.5 are yours). The
brief below is the task; it is committed at docs/dev-artifacts/prompts/crates/30-engine-layering-r3b.md.

# Brief: R3b — the migration module moves down into `table` (ENGINE-LAYERING R3b)

## Why

`db_instance/migration/` (the `MigrationCoordinator`, shadow log and shadow key) is constructed and read by table
code (`admitted_ops.rs` calls `MigrationCoordinator::new` and returns `Arc<MigrationCoordinator>`;
`read_only_views.rs` calls `MigrationCoordinator::load_state`), so `table -> db_instance` is a backward dependency
(3 files in the `rust-guard layers` ratchet baseline). The design moves the whole module into the table layer; the
coordinator's only in-crate dependencies are `crate::repo::{RepoWriteDomain, RepoWritePermit}` and
`crate::table::MigrationFreezeGuard`, both legal there.

## What (behaviour must not change; public paths must keep resolving)

1. Move the whole `db_instance/migration/` tree (`coordinator.rs`, `shadow_log.rs`, `shadow_key.rs`, `mod.rs`,
   `tests/`) into `table/table_manager/migration/` (`table/` and `table/support/` are full at 7/7; `table_manager/`
   has 6 entries and this slot is yours). Tests move with their module and stay declared.
2. `db_instance` keeps `pub use` of the module so `shamir_engine::db_instance::migration::*` keeps resolving, and the
   crate root keeps `shamir_engine::migration` (`lib.rs`: `pub use db_instance::migration;` stays valid, or point it at
   the new home; both public paths must still work). External users: `shamir-db`
   `execute/admin_db/admin_migration.rs` (`MigrationPhase`, `MigrationState`) and
   `tests/rename_migration/migrations/migration_index.rs` (`MigrationCoordinator`, `MigrationShadowLog`).
3. `admitted_ops.rs` and `read_only_views.rs` switch `crate::migration::` to the in-crate table path.
4. Visibility: items that must become `pub(crate)`/`pub` to keep the old paths are fine; list each widening in the
   final report.
5. Layers baseline: delete the `"table -> db_instance"` entry from `.rust-guard/layers_baseline.toml` when the pair
   reaches zero (the gate fails with `STALE_BASELINE` if you leave it). No other baseline edits.

## Scope and siblings

A sibling agent works in parallel on `TableCatalog` (R2a): files `repo/instance/repo_instance*.rs`,
`repo/instance/repo_instance/catalog*`, `repo/instance/support/catalog/fk_reverse_cache.rs` and a new module in
`table/support/storage/` or `table/support/guards/`. Do not touch those. Everything else of the engine layering
(`RepoCore`, tx sweeps, batch ports) is out of scope.

## Observable acceptance

`cargo fmt` and `cargo clippy -D warnings --all-targets` clean for `-p shamir-engine -p shamir-db`; layout passes;
the final report lists every moved/added/changed item. CI observes the rust-guard gates (structure, conservation,
modules, layers), rustdoc, engine-features, loom-model, test-discovery and the full matrix. Non-goals: R2 content,
behaviour change, crate creation, version bumps.

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
