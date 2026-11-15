You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
core-2c1-1882). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and in full
`docs/dev-artifacts/design/engine-layering.md` (sections 5.1, 5.6, 5.8, 6, 6.3, 6.4, 6.5, 6.6, 7.2 and ESPECIALLY
section 11 "As built", which records what the earlier stages did and what you need). The brief below is the task; it is
committed at docs/dev-artifacts/prompts/crates/37-engine-layering-r2c1.md.

# Brief: R2c1 — `RepoCore` and the thin `RepoInstance` shell (ENGINE-LAYERING R2c1)

## Why

`RepoInstance` (`repo/instance/repo_instance.rs`, 246 lines of fields and constructor) is the composition root of a
repository. 35 files of `tx` refer to it, and 11 of `query/batch` (the `rust-guard layers` ratchet in
`.rust-guard/layers_baseline.toml` counts them). Stages R2a (`TableCatalog`, table layer) and R2b (`DurabilityHandle`,
repo layer) have landed. This stage creates the aggregate that the lower layers will take INSTEAD of `&RepoInstance`:
`RepoCore`, a plain struct in the `tx` layer, built by the shell and passed down. The NEXT stage (a separate task
after yours lands) converts the 35 `tx` files from `&RepoInstance` to `&RepoCore`; another converts `query/batch`.
So your job is to make `RepoCore` COMPLETE: everything those files call today must be callable on `RepoCore`.

## What (behaviour must not change)

1. **`RepoCore` in the `tx` layer** (design 6.4; `tx/` is at the 7-entry limit, so create the module in a folder
   with a free slot or regroup with a reason; say where and why). A plain struct of the Arc cells that remain in
   `RepoInstance` after R2a/R2b, shared by clone: `name`, `commit_visibility_wait`, `repo: BoxRepo`, the
   `TableCatalog`, the `DurabilityHandle`, `tx_gate` (lazy cell), `tx_metrics`, `commit_tail`, `drainer` (the
   `OnceLock`), `receipt_worker`, `worker_lifecycle`, `replication_armed`, `wal_segment_bytes`, the FK reverse cache
   and the `cfg(test)` seam fields the moved methods need (`journal_publish_hook`, `table_init_hook`,
   `clean_horizon_test_state`, `background_drainer_disabled`). Group them into a `TxHorizon` bundle (`tx_gate`,
   `tx_metrics`, `commit_tail`, `drainer`, `commit_visibility_wait`: design 6.3) and keep the rest as `RepoCore`
   fields, as you judge cleanest. Same shared `Arc` cells, lazy `OnceCell` semantics untouched, nothing becomes
   eager, no new `dyn`.
2. **The FK reverse cache moves down.** Today `FkReverseCache` lives in `repo/instance/support/catalog/`
   (instance layer); a `tx`-layer struct holding it would be a backward edge. It depends only on the table-side
   `TableResolver` and `shamir_query_types`; move the type to the table layer (a folder with a free slot; the
   `crate::repo::fk_reverse_cache` path and `RepoInstance::fk_reverse_cache()` keep working through re-exports).
3. **The shell.** `RepoInstance` becomes `{ name, live, core: Arc<RepoCore> (or RepoCore, your call), the rename
   fence and its counter }`. The rename fence STAYS on the shell (its value types and `RenameFreeze::await_visible`
   take `&RepoInstance`; see section 11.1). `live` (the foreground liveness token) stays on the shell:
   background clones (`clone_for_background`, `live = None`) must keep the exact same semantics. Constructors
   (`from_factory`, `from_box_repo`, ...) build the core.
4. **Move the methods `tx` needs onto `RepoCore`**, leaving one-line delegating wrappers on the shell so that no
   `tx`, `query` or `shamir-db` call site changes in this stage. The census (design 11.3) of what the 35 `tx` files
   call: `table_by_token` (11 calls), `table_by_token_admitted`, `table_by_token_if_live`, `get_table`,
   `get_table_admitted`, `has_table`, `list_table_names`, `mapped_table_store_names`, `tx_info_store` (10),
   `tx_metrics` (21), `tx_gate` / `initialized_tx_gate` (19), `changefeed` (10), `drainer` (7), `commit_tail`,
   `commit_visibility_wait`, `write_domain` (10), `repo_wal`, `wal_group`, `wal_retention_gate`,
   `begin_repo_atomic_batch`, `storage_supports_atomic_batch`, `flush_all_history`, `settle_unwritten_version`,
   `prepare_journal_event`, `mark_changefeed_gap`, `record_pending_changefeed_gap`, `broadcast_changefeed_event`,
   `certify_cold_start` / `certify_legacy_cold_start`, `follower_journal_store`,
   `changelog_store_for_cold_reconcile`, `submit_receipt_job`, `initialized_mem_wal`, `note_unclean_open_bump`,
   `reserve_recovery_floor`, `wal_retention_refuse_writes`, `has_persistent_wal`, `per_table_mvcc`,
   `repo_interner`, `durability_policy`, `name`, the test hooks. Verify the list yourself (grep
   `repo.` / `self.repo.` call sites in `tx/**`) and complete it. Methods that need `BoxRepo`, the write domain or
   the tx gate belong on `RepoCore` (it now owns all of those). A method that really needs rename-fence or
   shell-only state stays on the shell; say which and why.
5. **Implicit batch bodies** (design 5.8): `run_implicit_batch_tx`, `begin_implicit_batch_tx`,
   `begin_implicit_batch_tx_with_isolation`, `commit_implicit_batch_tx` in `repo/instance/repo_instance/transactions.rs`
   move their bodies to free functions in a new `query/batch/implicit.rs` over `&RepoInstance`; the shell keeps
   one-line delegating methods (seven production call sites in `shamir-db` call them by name). `WriteResult`
   imports from `shamir_engine_query` directly. This removes the `instance -> query` reference except that one
   delegate (it is downward-legal).
6. **Single-init of the drainer** (design 6.3): the `OnceLock::get_or_init` closure that builds the drainer and
   spawns its task stays installed by the SHELL (it needs a background clone of the repo); `RepoCore` / `TxHorizon`
   only reads the cell. Prove in the final report, with the code lines, that the closure still runs exactly once
   under concurrent callers and that the drain task still holds a background clone (`live = None`) plus a `Weak` of
   the liveness token.
7. **Do not** convert the 35 `tx` files, `query/batch`, or `shamir-db` call sites: later stages. Do not change the
   `rename_fence` types.
8. `owned_impl.toml`: the inherent impls of `RepoCore` / `TxHorizon` live in their own defining module (no entry
   needed); keep the `RepoInstance` entry valid.

## Size and delivery

This is large. If the diff would exceed about 60 files, deliver it in TWO stages and STOP after the first with a
final report so that the orchestrator can land it: stage 1 = the structs, construction, shell wiring and the FK cache
move (methods still on the shell reading `self.core.<field>`), stage 2 = the methods onto `RepoCore` with delegating
wrappers plus the implicit-batch bodies. Each stage must compile and pass clippy on its own.

## Observable acceptance

`cargo fmt` and `cargo clippy -D warnings --all-targets` clean for `-p shamir-engine -p shamir-db` (and `-p
shamir-server -p shamir-sync` if any signature they use changed: it should not); layout passes; every public path
unchanged; the final report lists every moved/added/changed item and every widened visibility. CI observes the
rust-guard gates (the orchestrator fills a campaign delta from the findings), rustdoc, engine-features (the
`test-util` cells compile the cfg-gated constructors), loom-model, test-discovery and the full matrix. Non-goals:
the tx sweep, the batch ports, behaviour change, crate creation, version bumps.

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
