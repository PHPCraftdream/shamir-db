You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
small-edges-1877). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and in full
`docs/dev-artifacts/design/engine-layering.md` (the design this task executes; sections 5.9-5.11, 5.14, 5.16, 7.0,
7.3 and Appendix A are yours). The brief below is the task; it is committed at
docs/dev-artifacts/prompts/crates/28-engine-layering-r3.md.

# Brief: R3 "small edges" of the engine layering (ENGINE-LAYERING R3)

## Why

`crates/engine/shamir-engine` is being prepared for a split into layer crates (design document above). The module
layering gate `rust-guard layers` (config `.rust-guard/layers.toml`, ratchet `.rust-guard/layers_baseline.toml`) is
already in the repo; the baseline today (the gate's own count) holds these pairs that belong to this wave:
`repo -> table` 1 file, `repo -> instance` 5 files (3 `durability_policy` files belong to the sibling R2 wave, only
the 2 seed files are yours), `table -> instance` 2 files (`table_config.rs` is yours, `read_temporal.rs` belongs to
the sibling R1 wave), `table -> tx` 1 file.

## What (behaviour must not change; public paths must keep resolving)

1. **Seed orchestration up (design 5.14, 5.9):** `repo/seed/install/{inventory.rs,writer.rs}` and
   `repo/seed/prepare_seed.rs` move under `repo/instance/seed/`; the pure seed-install types
   (`install/receipt.rs`, `install/row.rs`, `SeedInstallState`) stay in `repo` (tx depends on them); the
   `install/tests/` dir splits with its subjects; `repo/mod.rs` seed re-exports rewire (type paths stay on `repo`,
   orchestration paths point at the instance module). `inventory.rs`'s `crate::table::TableConfig` import becomes a
   legal instance -> table reference.
2. **Store-name functions down (design 5.10, first bullet only):** `new_table_store_id` and
   `validate_table_store_name` (pure functions in `repo/instance/repo_instance/table_store/names.rs`, with
   `tests/names.rs`) move DOWN into `table/support/model/`; `table_store/mod.rs` re-exports them so the old path
   keeps resolving. The `read_temporal.rs` half (`observe_gate_with_budget`) belongs to R1; do not touch it.
3. **Meta canonical imports (design 5.16):** the six files that say `crate::meta::` switch to `crate::repo::meta::`
   (pure canonicalisation).
4. **Freeze-error split (design 5.11, Appendix A.1):** `table/table_manager/barriers/ddl_barriers.rs` gets a
   table-local `MigrationFreezeError` (`Storage(DbError)` and `MigrationFrozen{..}` with the same fields as
   `CommitError::MigrationFrozen`); `begin_migration_freeze` / `begin_migration_freeze_admitted` return it; the
   caller `repo/instance/repo_instance/catalog/rename_freeze.rs` maps it into its own flow; the facade keeps a thin
   public wrapper returning `CommitError`, because `shamir-db`
   (`execute/admin_db/admin_migration.rs`) calls `begin_migration_freeze_admitted`. Table-internal tests switch to
   the table-local error.
5. Config: when a pair of this wave reaches zero, delete its entry from `.rust-guard/layers_baseline.toml` (counts
   only go down); do not touch the entries of other waves.
6. rust-guard conservation campaign for this wave (design 7.0): `[[campaigns]]` entry in `.rust-guard/stages.toml`,
   a delta file under `.rust-guard/stages/` (`[[moved]]`, `[[changed]]`, `[[allow_widen]]`), `discovery_delta` for
   moved tests; tests are never declarable as lost. Derive entries from the shape documented in
   `tools/rust-guard/README.md` (you cannot run the binary here).

## Scope and siblings

NOT in this task: the `db_instance/migration/` move (a later task, because `read_only_views.rs` overlaps the R1
wave). A sibling agent works in parallel on R1 (table -> query/validator, `validator/engine_bound` fold,
`TableResolver`, `observe_gate_with_budget`) in another worktree; do not touch: the 19+9 table files named in design
5.3/5.5, `table/support/model/table_resolver.rs`, `validator/**`, `query/mod.rs`, `repo/instance/repo_instance/
catalog/visibility_impl.rs`, `read_only_views.rs`, `admitted_ops.rs`. A third wave (R2) starts later and owns the
`RepoInstance` knot.

## Observable acceptance

`cargo fmt -p shamir-engine` clean; `cargo clippy -p shamir-engine -p shamir-db --all-targets -- -D warnings` clean;
layout passes; read EVERY changed file once more for the whole class of error (module paths after a move,
`cfg(test)` gating, unused imports, privacy) before the final report, because the fast tier stops at the first
failing crate. CI observes the rust-guard gates, rustdoc, engine-features, loom-model, test-discovery (moved tests)
and the full matrix. Non-goals: R1/R2 content, behaviour change, crate creation, version bumps.

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
