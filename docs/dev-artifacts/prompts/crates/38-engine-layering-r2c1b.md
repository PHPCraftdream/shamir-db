# Brief: R2c1 stage 2 — the methods of `RepoCore` and the implicit-batch bodies (ENGINE-LAYERING R2c1b)

## Why

Stage 1 of R2c1 (the parent commit of this branch) created `RepoCore` and `TxHorizon` in `tx/commit/core/`, reduced
`RepoInstance` to the shell `{ name, live, core, rename_fence, rename_fence_len }` and moved the FK reverse cache
type to the table layer. Every shell method still does its own work, reading through `self.core.*` /
`self.core.horizon.*` accessors. The next two stages convert the 35 `tx` files and `query/batch` from
`&RepoInstance` to `&RepoCore`; that is only possible when everything those files call is callable on `RepoCore`.
This stage makes `RepoCore` complete. Behaviour must not change.

## What

1. **Methods onto `RepoCore`.** The census (design section 11.3) of what the 35 `tx` files call on the repo:
   `table_by_token`, `table_by_token_admitted`, `table_by_token_if_live`, `get_table`, `get_table_admitted`,
   `has_table`, `list_table_names`, `mapped_table_store_names`, `tx_info_store`, `tx_metrics`, `tx_gate` /
   `initialized_tx_gate`, `changefeed`, `drainer` (the accessor only, see 4), `commit_tail`, `commit_visibility_wait`,
   `write_domain`, `repo_wal`, `wal_group`, `wal_retention_gate`, `begin_repo_atomic_batch`,
   `storage_supports_atomic_batch`, `flush_all_history`, `settle_unwritten_version`, `prepare_journal_event`,
   `mark_changefeed_gap`, `record_pending_changefeed_gap`, `broadcast_changefeed_event`,
   `certify_cold_start` / `certify_legacy_cold_start`, `follower_journal_store`,
   `changelog_store_for_cold_reconcile`, `submit_receipt_job`, `initialized_mem_wal`, `note_unclean_open_bump`,
   `reserve_recovery_floor`, `wal_retention_refuse_writes`, `has_persistent_wal`, `per_table_mvcc`,
   `repo_interner`, `durability_policy`, `name`, the `cfg(test)` hooks. VERIFY the list yourself: grep the
   `repo.` / `self.repo.` call sites in `tx/**` and complete it (this stage must not leave a callee off the list).
   For each: the BODY moves to an inherent `impl RepoCore` / `impl TxHorizon` in `tx/commit/core/`, and the shell
   keeps a one-line delegating method with the same name, signature and visibility, so that no `tx`, `query` or
   `shamir-db` call site changes. Group the impl blocks into topical files next to `repo_core.rs` (`tx/commit/core/`
   has 3 entries, so room for 4 more; e.g. catalog access, durability access, journal access, tx access; one file =
   one coherent group, at most about 400 lines each). Move callees before callers.
2. **A method stays on the shell, or is only partly moved, when** it needs shell-only state (`live`, `rename_fence`,
   `rename_fence_len`, `clone_for_background`, `with_name`, anything taking `&RepoInstance` in a value type), or when
   its body refers to code of the `instance` or `query` layers (see Layer rules). Say in the final report which methods
   stayed and why.
3. **Implicit-batch bodies.** `run_implicit_batch_tx`, `begin_implicit_batch_tx`,
   `begin_implicit_batch_tx_with_isolation` and `commit_implicit_batch_tx` in
   `repo/instance/repo_instance/transactions.rs` move their bodies to free functions over `&RepoInstance` in a new
   file `query/batch/control/implicit.rs` (`query/batch/` is full at 7 entries; `query/batch/control/` has room;
   register it in `control/mod.rs` with a re-export only). The shell keeps one-line delegating methods (seven
   production call sites in `shamir-db` call them by name). `WriteResult` imports from `shamir_engine_query`
   directly. Keep the doc comments that explain the compile-time inlining concern of these functions.
4. **The drainer stays as it is.** The `OnceLock::get_or_init` closure that builds the drainer and spawns its task stays
   installed by the SHELL (it needs a background clone with `live = None` plus a `Weak` of the liveness token);
   `RepoCore` / `TxHorizon` only read the cell (`drainer_cell()`). Do not move that closure. State in the final report,
   with file and line, that it is unchanged.
5. **Owned impls.** Add `[[owned_impl]]` entries to `.rust-guard/owned_impl.toml` for `RepoCore` and `TxHorizon`
   (module `crate::tx::commit::core`), so that every inherent impl of them must live there.
6. **Do not** convert the 35 `tx` files, `query/batch` or `shamir-db` call sites (later stages), do not change the
   `rename_fence` types, do not create crates, do not touch versions.

## Layer rules (the `rust-guard layers` ratchet fails on any new backward pair; you cannot run it here)

`repo < table < validator < tx < query < instance < db_instance`; a module may refer only to its own and lower layers.
`RepoCore` is in `tx`: its methods may use `repo`, `table`, `validator` and `tx` items only. A body that names a
`query::` or `repo::instance` item (for example `BatchResolver`, `RepoInstance`, `query::batch`) cannot move onto
`RepoCore`; leave it on the shell or on the `query` side. Re-exporting a lower-layer item from a higher-layer
`mod.rs` is fine, but a `repo/mod.rs` re-export of a `table` item is a backward pair (stage 1 hit exactly this and
fixed it): do not add re-exports from `repo/` of table, tx or query items. Re-read each moved body for every path it
names before leaving it on `RepoCore`.

## Lessons (binding)

* No function-local `use` in any file you touch (the structure gate flags every local `use` in a changed file, also
  old ones): hoist them to the file header.
* No intra-doc links of the form `[`x`](Self::x)` to an item that moved to another type: use plain backticks.
  `cargo doc` with broken links promoted to errors runs in CI and you cannot run it here; re-read every doc comment
  of a moved item.
* Code under `cfg(test)`, `cfg(feature = ...)` and `cfg(any(test, feature = "test-util"))` is compiled by your local
  clippy only partly: re-read every changed file for unused imports, wrong paths and privacy in such code.
* Delegation: at most two parallel workers at a time, on disjoint files (the shared rush store is contended; an
  executor that dies on a store timeout is resumed, it is not a failure).
* Create the campaign entry in `.rust-guard/stages.toml` (name `engine-layering-R2c1b`, `packages = ["shamir-engine"]`)
  and the two delta files `.rust-guard/stages/engine-layering-R2c1b.toml` and `...-discovery.toml` containing ONLY
  `base = "<the sha of this branch's merge base with origin/master>"`; the orchestrator fills them from the CI
  findings. Replace the stage-1 campaign entry (`engine-layering-R2c1`) and its two delta files: the orchestrator
  re-bases this branch after stage 1 lands, so leave a single campaign entry for this stage.

## Observable acceptance

`cargo fmt -p shamir-engine -p shamir-db -- --check` and `cargo clippy -p shamir-engine -p shamir-db --all-targets
-- -D warnings` clean; `python -I scripts/ci/check_layout.py` OK; every public path unchanged; no `tx`, `query` or
`shamir-db` call site changed except those named in 3; the final report lists every moved / added / changed item, every
widened visibility, the methods that stayed on the shell with the reason, and the drainer proof of 4. CI observes the
rust-guard gates (the orchestrator fills the campaign delta from the findings), `layers`, rustdoc, engine-features,
loom-model and the full matrix. Non-goals: the `tx` sweep, the batch ports, behaviour change, crate creation, version
bumps.
