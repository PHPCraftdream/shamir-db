You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
table-faces-1875). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and in full
`docs/dev-artifacts/design/engine-layering.md` (the design this task executes; sections 5.3-5.5, 7.0, 7.1 and
Appendix A are yours). The brief below is the task; it is committed at
docs/dev-artifacts/prompts/crates/27-engine-layering-r1.md.

# Brief: R1 "table faces outward" — dissolve table -> query/validator edges (ENGINE-LAYERING R1)

## Why

`crates/engine/shamir-engine` is being prepared for a split into layer crates (design document above). The module
layering gate `rust-guard layers` (config `.rust-guard/layers.toml`, ratchet `.rust-guard/layers_baseline.toml`) is
already in the repo. In the baseline today (the gate's own count; re-exports are attributed to the module where the
item really lives) these pairs belong to this wave: `table -> validator` 9 files, `table -> query` 5 files
(`RowByteMeter::value_size` in `collect.rs`, and `crate::query::TableResolver` in the four write exec files),
`validator -> query` 1 file (`validator_db.rs`), and 1 of the 2 `table -> instance` files
(`read_temporal.rs`, via `observe_gate_with_budget`). A further 19 `table` files import `crate::query::{filter,read,
write}`, which is only a re-export of the separate crate `shamir-engine-query`; the gate does not count them, but
`table` must stop going through that facade because it becomes its own crate later.

## What (behaviour must not change; public paths must keep resolving)

1. Rewrite the 19 leaf-shadow `table` files (list in design 5.3) to import `shamir_engine_query::...` directly.
2. `RowByteMeter::value_size` moves into `shamir-engine-query`'s read module (the leaf owns `QueryRecord`);
   `exec_scope.rs` and `arms_read.rs` import it from there (design 5.4).
3. Split `TableResolver` (design 5.4.1): the table-scoped part becomes a trait in
   `table/support/model/table_resolver.rs` (you may choose the file name within `table/support/model/`);
   `executor_traits.rs` keeps the repo-scoped part (`resolve_repo`) and re-exports the table-side trait so
   `crate::query::batch::executor_traits::TableResolver` and `shamir_engine::query::TableResolver` keep resolving
   (`query/mod.rs` re-export source switches). Dispatch shape must not change: keep generics where callers hold the
   trait generically; no new `dyn` on write/read hot paths.
4. The 9 `table` files that use validators import `shamir_engine_validator::...` directly for the portable items;
   fold `validator/engine_bound/{mod.rs,persistence.rs,validator_db.rs}` and its `tests/` into
   `table/validator_bound/`; `validator/mod.rs` re-exports from the new home so `shamir_engine::validator::{ValidatorDb,
   ValidatorFkCtx, BatchFkCache, FkCacheEntry, persistence}` keep resolving (design 5.5); `validator_db.rs` switches
   `TableRef` to `shamir_query_types::TableRef` and `TableResolver` to the table-side trait.
5. `observe_gate_with_budget` moves down into `table` (design 5.10, second bullet only: the `read_temporal.rs`
   half; the store-name functions belong to the sibling R3 task and must NOT be touched); `repo/mod.rs`'s
   `visibility_observer` re-export points at the new home.
6. Config: when `table -> validator`, `table -> query`, `validator -> query` are zero, delete their entries from
   `.rust-guard/layers_baseline.toml`; when nothing is left in the `validator` layer (the folded module is gone),
   remove it from the layer list in `.rust-guard/layers.toml` and from the baseline. Counts may only go down.
7. rust-guard conservation campaign for this wave (design 7.0): `[[campaigns]]` entry in `.rust-guard/stages.toml`,
   delta file under `.rust-guard/stages/` with `[[moved]]` / `[[changed]]` / `[[allow_widen]]` entries and a
   `discovery_delta` for the moved tests. Use `rust-guard ... --suggest-delta` output as a starting draft (read it;
   you cannot run the binary here, so derive the entries by reading the shape in `tools/rust-guard/README.md`).
   Tests are never declarable as lost: every moved test must be a `[[moved]]` entry.

## Scope and siblings

Your file set is exactly the one named in design 7.1. A sibling agent works in parallel on R3 (seed move, store-name
functions, meta canonical imports, freeze-error split) in a different worktree; do not touch its files:
`repo/seed/**`, `table/support/model/table_config.rs`, `repo/instance/repo_instance/table_store/**`,
`repo/meta/**`, `table/table_manager/barriers/ddl_barriers.rs`, `repo/instance/repo_instance/catalog/rename_freeze.rs`,
`db_instance/migration/**`. `read_only_views.rs` is yours (import switch for the leaf and validator items only);
its `crate::migration::` reference is left alone (a later task moves migrations).

## Observable acceptance

`cargo fmt -p shamir-engine -p shamir-engine-query` clean; `cargo clippy -p shamir-engine -p shamir-engine-query
-p shamir-engine-validator -p shamir-db --all-targets -- -D warnings` clean (public paths preserved means dependants
compile; clippy type-checks tests and benches too); layout passes; read EVERY changed file once more for the whole
class of error (module paths after a move, `cfg(test)` gating, unused imports, privacy) before the final report,
because the fast tier stops at the first failing crate. CI observes: rust-guard structure/conservation/modules/layers
gates, rustdoc, engine-features, loom-model, test-discovery (moved tests), full matrix. Non-goals: R2/R3 content, any
behaviour change, crate creation, version bumps.

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
