You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
catalog-2a-1876). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and in full
`docs/dev-artifacts/design/engine-layering.md` (sections 5.8, 6, 6.1, 6.7, 7.2 "R2a", Appendix A are yours). The
brief below is the task; it is committed at docs/dev-artifacts/prompts/crates/29-engine-layering-r2a.md.

# Brief: R2a — `TableCatalog` out of `RepoInstance` (ENGINE-LAYERING R2a)

## Why

`RepoInstance` (`repo/instance/repo_instance.rs`) is the composition root of a repository and the knot of the engine's
module graph: 35 files of `tx`, 11 of `query/batch` and 3 of `repo` refer to it (the `rust-guard layers` ratchet in
`.rust-guard/layers_baseline.toml` counts them: `tx -> instance` 35, `query -> instance` 11, `repo -> instance` 5, 2 of
which are fixed by an earlier wave). The design splits it into plain-struct bundles owned by lower layers plus a thin
shell. This task is the FIRST stage (R2a): the table catalogue. Later stages (R2b durability, R2c horizon and core,
R2d batch ports, the tx sweeps) run after this one lands and edit `repo_instance.rs` after you, so keep this stage
self-contained.

## What (behaviour must not change)

1. New plain struct `TableCatalog` in the table layer (design 6.1) with the Arc fields of `RepoInstance` that
   make up the catalogue: `configs`, `tables`, `token_names`, `per_table_mvcc`, `rename_fence` (with
   `rename_fence_len`), `repo_interner` (read the struct to get the exact field list and types; clones are shallow
   `Arc` clones). Placement: `table/` and `table/support/` are full (7/7) and `table/table_manager/` is reserved for
   a parallel task; put the module in a folder with a free slot (for example `table/support/storage/` or
   `table/support/guards/`, 5/7 each) or regroup with a reason, and say where and why in the report.
2. The inherent methods that implement the catalogue move onto `TableCatalog`: from
   `repo/instance/repo_instance/catalog.rs` and `catalog/repo_membership_impl.rs` (`table_by_token`,
   `table_by_token_if_live`, `get_table`, `get_table_admitted`, `get_table_unfenced`, `has_table`, `list_table_names`,
   `mapped_table_store_names`, `table_count`, and whatever else only touches these fields; read the files and decide
   method by method; a method that also needs repo-wide state stays on `RepoInstance` and calls the catalogue).
   `RepoInstance` keeps every public method as a one-line delegating wrapper, so `owned_impl.toml` stays valid
   (the moved methods become inherent impls of `TableCatalog` in its own module: no entry needed).
3. Lazy `OnceCell` semantics: the catalogue holds the SAME `Arc<OnceCell<...>>` cells (clone the Arc); table
   initialisation stays where it is today, nothing becomes eager.
4. `repo/instance/support/catalog/fk_reverse_cache.rs`: its `crate::query::batch::TableResolver` and
   `crate::query::TableRef` references switch to the table-side trait `crate::table::TableResolver` (it lives in
   `table/support/table_resolver.rs`) and `shamir_query_types::TableRef`. (If the cache needs `resolve_repo` it uses the
   `BatchResolver` port in `query/batch/exec/executor_traits.rs`; that edge is downward-legal.)
5. Hot paths stay static: plain structs and generics, no new `dyn`.
6. Layers baseline: the number of files in the `tx -> instance` and `query -> instance` pairs must not grow; do not
   touch the config otherwise (the orchestrator trues it up from the gate output).

## Scope and siblings

A sibling agent works in parallel on a different task: moving `db_instance/migration/` into
`table/table_manager/migration/` (files: `db_instance/**`, `table/table_manager/admission/domain/barrier/admitted_ops.rs`,
`table/table_manager/admission/views/read_only_views.rs`, `table/table_manager.rs` / `table/mod.rs` only for the one
`migration` declaration/re-export). Do not touch those. Later stages (durability handle, `TxHorizon`, `RepoCore`, the
tx sweeps, the batch ports) are NOT yours: do not convert the 35 `tx` files, do not create `RepoCore`.

## Observable acceptance

`cargo fmt` and `cargo clippy -D warnings --all-targets` clean for `-p shamir-engine -p shamir-db`; layout passes;
public paths unchanged; the final report lists every moved/added/changed item. CI observes the rust-guard gates
(structure, conservation, modules, layers), rustdoc, engine-features, loom-model, test-discovery and the full matrix.
Non-goals: any later stage, behaviour change, crate creation, version bumps.

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
