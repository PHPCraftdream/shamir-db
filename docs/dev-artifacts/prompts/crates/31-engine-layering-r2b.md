You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
durability-2b-1881). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and in full
`docs/dev-artifacts/design/engine-layering.md` (sections 5.2, 5.14, 6, 6.2, 6.7, 7.2 "R2b", Appendix A are yours). The
brief below is the task; it is committed at docs/dev-artifacts/prompts/crates/31-engine-layering-r2b.md.

# Brief: R2b — `DurabilityHandle` out of `RepoInstance` (ENGINE-LAYERING R2b)

## Why

`RepoInstance` (`repo/instance/repo_instance.rs`) is the composition root of a repository and the knot of the engine's
module graph (`rust-guard layers` ratchet in `.rust-guard/layers_baseline.toml`: `tx -> instance` 35, `query -> instance`
11, `repo -> instance` 5, two of which were fixed by earlier waves, so 3 remain: the `durability_policy` files).
The design splits it into plain-struct bundles owned by lower layers plus a thin shell. Stage R2a (`TableCatalog`,
in `table/support/storage/catalog.rs`) has landed; this is stage R2b: the durability handle. Later stages (R2c horizon
and core, the tx sweeps, the batch ports) edit `repo_instance.rs` after you.

## What (behaviour must not change)

1. New plain struct `DurabilityHandle` in the repo layer (design 6.2, in the journal backend module of `repo/`), a
   bundle of the durability-related Arc fields of `RepoInstance`: the WAL cells (`repo_wal`, `wal_group`),
   `group_commit`, the changefeed cell and `pending_changefeed_gap`, the `durability` policy ArcSwap,
   `durability_metrics`, `wal_retention_gate`, `write_domain`, `marker_write_mutex`, `wal_dir` (read the struct for
   the exact fields and types). Same shared `Arc` cells (clone the `Arc`s); lazy `OnceCell` semantics unchanged:
   initialisation stays where it is, the handle only holds the same cells. No new `dyn`, nothing becomes eager.
2. The methods that implement the durability surface and only need those fields move onto `DurabilityHandle`:
   from `repo/instance/repo_instance/lifecycle/journal.rs` (`prepare_journal_event`, mark/record the pending
   changefeed gap, `broadcast_changefeed_event`, the bounded journal read), `lifecycle/durability_repo_impl.rs`
   (`repo_wal`/`wal_group`/`durability` accessors, `settle_unwritten_version`, `note_unclean_open_bump`),
   `lifecycle/clean_horizon_repo_impl.rs` (`wal_retention_gate`, `wal_retention_refuse_writes`,
   `reserve_recovery_floor`), `transactions/follower_durability.rs` (`follower_journal_store`) and
   `cold_reconcile_repo_impl.rs` (`changelog_store_for_cold_reconcile`). Decide method by method: a method that also
   needs repo-wide state stays on `RepoInstance` and calls the handle. `RepoInstance` keeps every public/crate method
   as a one-line delegating wrapper, so call sites in `tx` and `shamir-db` do not change in this stage and
   `owned_impl.toml` stays valid.
3. `repo/journal/backend/durability_policy/{commit_policy,recovered_open,replication_page}.rs` take
   `&DurabilityHandle` (plus the repo name where they need it) instead of `&RepoInstance`; this zeroes
   `repo -> instance` for those three files.
4. Do not touch the 35 `tx` files, `query/batch`, `RepoCore`, `TxHorizon`: later stages. The rename fence stays on
   `RepoInstance` (see the R2a pull request for why).
5. Layers baseline: when `repo -> instance` reaches zero, delete the entry from `.rust-guard/layers_baseline.toml`
   (the gate fails with `STALE_BASELINE` otherwise); do not touch other entries; the `tx -> instance` and
   `query -> instance` counts must not grow.

## Scope and siblings

No parallel sibling at the time of writing; later stages wait for this one.

## Observable acceptance

`cargo fmt` and `cargo clippy -D warnings --all-targets` clean for `-p shamir-engine -p shamir-db`; layout passes;
public paths unchanged; the final report lists every moved/added/changed item and every widened visibility. CI
observes the rust-guard gates, rustdoc, engine-features, loom-model, test-discovery and the full matrix.
Non-goals: later stages, behaviour change, crate creation, version bumps.

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
