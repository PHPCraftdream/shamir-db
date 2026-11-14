# Engine Layering: Splitting `shamir-engine` into Layer Crates

- Status: design — decisions fixed; wave execution follows in separate tasks
- Date: 2026-11-14; worktree `engine-layers-1872` at commit `3c20d01b8`
- Scope: `crates/engine/shamir-engine` — corrected layer order, target crate layout, backward-edge break plan, `RepoInstance` decomposition, wave plan, `layers.toml` gate proposal
- Companion brief: `docs/dev-artifacts/prompts/crates/26-engine-layers-design.md`
- This document changes no source. All numbers are measured on this worktree (see §2).

## 1. Summary

`crates/engine/shamir-engine` (61,038 production / 129,614 test src lines by this document's src-only count) is a single crate whose internal modules form a partially cyclic dependency graph held together by re-export shadows and the `RepoInstance` god-type. This design (a) corrects the layer order to `repo < table < tx < query(batch) < repo/instance < db_instance` — swapping the brief's candidate `query < tx` after re-export shadows are discounted, (b) dissolves `validator` as a layer (portable validators already live in `shamir-engine-validator`; the engine-bound glue folds into `table`), and (c) splits the engine group from 4 crates to exactly the group cap of 7 by extracting `shamir-engine-repo` (~4.1k prod lines), `shamir-engine-table` (23.3k), and `shamir-engine-tx` (15.6k), leaving `shamir-engine` as a facade (16.7k) that keeps query/batch, the `RepoInstance` shell, `db_instance`, and every existing public path. `shamir-engine-query`, `shamir-engine-validator`, and `shamir-db` are unchanged. The work is delivered in four non-overlapping waves (R1–R3 parallelizable in bounded ways, R4 last) plus a proposed `.rust-guard/layers.toml` ratchet schema whose gate is a separate task.

**Corrected layer order (bottom → top):** `repo < table < tx < query(batch) < repo/instance < db_instance`.
**End state:** 7 engine-group crates — `shamir-engine-repo`, `shamir-engine-table`, `shamir-engine-tx`, `shamir-engine` (facade), plus unchanged `shamir-engine-query`, `shamir-engine-validator`, `shamir-db`.

## 2. Measurement basis

**Method.** Counts are **files** (not reference occurrences) over production `src/**/*.rs` of `crates/engine/shamir-engine`: `tests/` dirs and `#[cfg(test)]` code excluded; brace-grouped `use crate::{...}` imports included. Measured 2026-11-14 on worktree commit `3c20d01b8`. Where the task brief's numbers differ, the difference is the counting convention (the brief's 62,573 / 142,808 include benches, examples, and integration tests; this document uses its own src-only numbers consistently). Backward pairs are counted against the **corrected** order (§3), with re-export shadows attributed to their true source (§2.3).

### 2.1 Module sizes (src only)

| module (in-crate today) | prod lines | test lines |
|---|---:|---:|
| `table/` (80 prod files) | 23,285 | 50,225 |
| `tx/` | 15,603 | 36,894 |
| `query/batch/` + facade `query/mod.rs` (454 lines) | 10,208 | 28,224 |
| `repo/` without `instance/` and without `seed/` install orchestration ("repo-low") | 4,122 | 10,037 |
| `repo/instance/` | 5,078 | 245 |
| `validator/` in-crate part = `validator/engine_bound/` only (`mod.rs`, `persistence.rs`, `validator_db.rs`) | 807 | 2,003 |
| `db_instance/` (incl. `migration/`) | 1,440 | 1,986 |
| root `lib.rs` | 41 | 0 |
| **total** | **61,038** | **129,614** |

Notes: the evaluation leaf is **already** the separate crate `shamir-engine-query`; in-crate `query` is only `batch/` plus a facade `mod.rs` that re-exports `shamir_engine_query::query::{admin,auth,filter,read,write}` (`src/query/mod.rs:435-453`). The portable validators already live in `shamir-engine-validator` (schema/codecs/registry/execution), re-exported via `pub use shamir_engine_validator::validator::*`.

### 2.2 Churn (files changed since 2026-09-01, whole crate = 835 src files)

table 281 · tx 231 · query 148 · repo 107 (of which `repo/instance` 38) · db_instance 16 · validator 13 · benches 48 · integration tests 13.

### 2.3 Backward pairs (adoption baseline, corrected order)

"Upper → lower" = referencing module → referenced module. "Shadow" = the reference resolves through a facade re-export to an item that actually lives elsewhere (leaf crate, separate crate, or another module).

| # | pair | files | nature |
|---|---|---:|---|
| 1 | tx → repo/instance | 35 | real (`RepoInstance`) |
| 2 | tx → repo-low | 12 | real, **downward-legal** under corrected order |
| 3 | table → query leaf | 19 | re-export shadow |
| 4 | table → query batch | 5 | 1 shadow helper + `TableResolver` |
| 5 | table → validator | 9 | half shadow, half engine-bound glue |
| 6 | query/batch → tx | 3 | real |
| 7 | query/batch → repo/instance | 3 | real |
| 8 | repo/instance → query batch | 2 | real (plus 1 leaf-shadow ref + 1 doc link) |
| 9 | repo(seed) → table | 1 | real |
| 10 | table → repo/instance | 2 | real |
| 11 | table → tx | 1 | real |
| 12 | tx → query leaf | 1 | re-export shadow |
| 13 | validator(engine_bound) → query | 1 | shadow + real mix |
| 14 | repo-low → repo/instance | 5 | real (root holders) |
| 15 | table → db_instance (migration alias) | 1 | alias artifact, real direction |
| 16 | `crate::meta` alias artifacts | 6 | directionally legal; canonical-import cleanup |

**Pair 1 — tx → repo/instance: 35 files.** All reference `RepoInstance` (composition root, `repo/instance/repo_instance.rs:41-174`; inherent impl blocks spread over 17 files under `repo/instance/repo_instance/`, pinned by `.rust-guard/owned_impl.toml` entry `[[owned_impl]] type = "RepoInstance" module = "crate::repo::instance::repo_instance"`):

```
tx/commit/acknowledgement/tail.rs                       tx/commit/pipeline/driver/commit_driver.rs
tx/commit/pipeline/phase/commit/entry.rs                tx/commit/pipeline/phase/commit/legacy_and_lockfree.rs
tx/commit/pipeline/phase/commit_phases.rs               tx/commit/pipeline/phase/finalize.rs
tx/commit/pipeline/phase/materialize.rs                 tx/commit/pipeline/phase/publish_journaled.rs
tx/drain/drainer/drain_step.rs                          tx/drain/drainer/orphan_repair/missing_evidence.rs
tx/drain/drainer/orphan_repair/repair.rs                tx/drain/drainer/orphan_repair/settle.rs
tx/drain/drainer/truncate.rs                            tx/preparation/pre_commit/base_index.rs
tx/preparation/pre_commit/claims.rs                     tx/preparation/pre_commit/index2.rs
tx/preparation/pre_commit/locked.rs                     tx/preparation/pre_commit/prelock.rs
tx/preparation/pre_commit/stale_value.rs                tx/recovery/clean_horizon/checkpoint.rs
tx/recovery/clean_horizon/recovery_floor.rs             tx/recovery/clean_horizon/repair_pending_drain.rs
tx/recovery/clean_horizon/retention.rs                  tx/recovery/cold_reconcile/algorithm.rs
tx/recovery/cold_reconcile/receipt_recovery.rs          tx/recovery/v2.rs
tx/replication/apply_replicated.rs                      tx/replication/follower_durability/receipt/apply.rs
tx/replication/follower_durability/receipt/checkpoint.rs
tx/replication/follower_durability/receipt/publish.rs
tx/replication/follower_durability/receipt/resume/activation.rs
tx/replication/follower_durability/receipt/resume/identity.rs
tx/replication/follower_durability/receipt/resume/lookup.rs   (param entirely unused — dropped)
tx/replication/follower_durability/receipt/resume/worker.rs   (owns a background clone)
tx/replication/follower_durability/receipt/stage.rs
```

**Pair 2 — tx → repo-low: 12 files** (true count; `tail.rs`'s `crate::repo::visibility_observer` import is an *instance* item misattributed to repo-low and is excluded here). Ref-occurrences: `write_domain` ×9, `RepoWritePermit` ×3, plus `StoreChangelog`, `SeedInstallState`, `checkpoint_allocation_floor`, `visibility_observer`. Under the corrected order this pair is **downward-legal**: it becomes the `shamir-engine-tx → shamir-engine-repo` crate dependency at R4 with no code change beyond the R2b handle-parameter fix (§5.14, §6).

**Pair 3 — table → query leaf: 19 files** (shadow; every one imports `crate::query::{filter,read,write,...}` = `pub use shamir_engine_query::query::{...}`, `src/query/mod.rs:435-453`): `table/read/dispatch/{collect,explain,paged,scan,vector}.rs`; `table/read/execution/{candidate_set,dispatch}.rs`; `table/read/planning/{read_asof_seek,read_index_scan,read_planner,read_temporal}.rs`; `table/support/model/table.rs`; `table/table_manager/accessors/table_manager_streaming.rs`; `table/table_manager/admission/views/read_only_views.rs`; `table/write/delete/execute.rs`; `table/write/insert.rs`; `table/write/set/execute.rs`; `table/write/update/execute.rs`; `table/write/write_helpers.rs`.

**Pair 4 — table → query batch: 5 files.** `collect.rs` uses `crate::query::batch::exec_scope::RowByteMeter::value_size`; the 4 write exec files (`write/{delete/execute,insert,set/execute,update/execute}.rs`) use `crate::query::TableResolver` (defined in `query/batch/exec/executor_traits.rs`).

**Pair 5 — table → validator: 9 files** (`table_manager.rs`, `table_manager/accessors/table_manager_validators.rs`, `table_manager/admission/domain/barrier/write_domain_barrier.rs`, `table_manager/admission/views/read_only_views.rs`, `write/delete/execute.rs`, `write/insert.rs`, `write/set/execute.rs`, `write/update/execute.rs`, `write_helpers.rs`). Split: portable re-exports (`WriteOp`, `ValidatorRegistry`, `ValidatorBinding`, `ValidatorFailure`, `RecordValidator`, `RecordFields`, `ForeignKeyRef`, `TransformSpec` — all from `shamir-engine-validator` via the star re-export; `WriteOp` is defined nowhere in shamir-engine src) vs engine-bound glue (`ValidatorFkCtx`, `BatchFkCache`, `validator::persistence::{save_validators_metadata, load_validators_metadata}` — defined in `validator/engine_bound/{validator_db.rs, persistence.rs}`).

**Pair 6 — query/batch → tx: 3 files.** `receipts/commit.rs` (`use crate::tx::ack::commit_tx_with_kind`, `use crate::tx::CommitError`; holds `repo: &'a RepoInstance`; also the pure `CommitError`→wire-code mapping `receipt_error_code`); `exec/batch_execute.rs` (no direct tx-type imports; calls repo methods `synced_flush`/`sync_wal`/`commit_visibility_wait`/`commit_tx_with_options` and `crate::tx::release_pessimistic_locks(&tx, &repo)`); `exec/interactive_tx.rs` (`repo.begin_tx(iso)`, `repo.commit_tx(tx)`; names `crate::tx::{TxOutcome, CommitError}` as types only).

**Pair 7 — query/batch → repo/instance: 3 files.** `receipts/commit.rs`, `interactive_tx.rs`, `exec/executor_traits.rs` (its trait `TableResolver` has `async fn resolve_repo(&self, repo_name: &str) -> DbResult<crate::repo::RepoInstance>`; the file also imports `crate::table::TableManager` and defines `AdminExecutor`, `FunctionInvoker`).

**Pair 8 — repo/instance → query: 2 files with real refs** (`repo_instance/transactions.rs`: `crate::query::batch::BatchError`, `crate::query::batch::receipts::commit_implicit`, `crate::query::write::WriteResult`; `support/catalog/fk_reverse_cache.rs:54-55`: `crate::query::batch::TableResolver`, `crate::query::TableRef`) plus one doc-comment-only reference (`repo_instance.rs:39`, `[`crate::query::batch::Authorized`]`). `crate::query::TableRef` is itself a shadow of `shamir_query_types::TableRef`.

**Pair 9 — repo(seed) → table: 1 file.** `repo/seed/install/inventory.rs` (`use crate::table::TableConfig`).

**Pair 10 — table → repo/instance: 2 files.** `table/support/model/table_config.rs` (`use crate::repo::instance::repo_instance::table_store::{new_table_store_id, validate_table_store_name}` — two pure fns in `repo/instance/repo_instance/table_store/names.rs`, with `tests/names.rs`); `table/read/planning/read_temporal.rs` (`use crate::repo::visibility_observer::observe_gate_with_budget` — the fn lives in `repo/instance/repo_instance/catalog/visibility_impl.rs` (≈:169), re-exported at `repo/mod.rs:13`; operates on shamir-tx gate types + a budget, no `RepoInstance` state).

**Pair 11 — table → tx: 1 file.** `table/table_manager/barriers/ddl_barriers.rs` (`use crate::tx::CommitError`; `begin_migration_freeze` returns `Result<MigrationFreezeGuard, CommitError>`, maps `admit()` error to `CommitError::Storage`, builds `CommitError::MigrationFrozen{..}`).

**Pair 12 — tx → query leaf: 1 file.** `tx/preparation/predicate_range.rs` (`use crate::query::filter::eval::intern_field_path; use crate::query::filter::{Filter, FilterValue}` — shadow artifact).

**Pair 13 — validator(engine_bound) → query: 1 file.** `validator_db.rs`: `crate::query::TableRef` (shadow of `shamir_query_types::TableRef`) and `crate::query::TableResolver` (real — `query/batch/exec/executor_traits.rs`).

**Pair 14 — repo-low → repo/instance: 5 files** holding/using the root: `repo/seed/install/writer.rs` (SeedImageWriter owns a `RepoInstance` clone), `repo/seed/prepare_seed.rs` (quiesce/drain/flush orchestration; calls the shell method `repo.flush_seed_metadata()` at :80), `repo/journal/backend/durability_policy/durability_policy/commit_policy.rs` (reads `repo.name()`/`repo.durability()`), `.../recovered_open.rs` (opens repo.repo store, gate init), `.../replication_page.rs` (wal_group + bounded changelog read); plus the `pub use repo_instance::RepoInstance` line at `repo/mod.rs:35`.

**Pair 15 — table → db_instance: 1 file.** `table/table_manager/admission/domain/barrier/admitted_ops.rs:25` `use crate::migration::{MigrationCoordinator, MigrationShadowLog, MigrationState}` — `crate::migration` is `pub use db_instance::migration` (`lib.rs:27`).

**Pair 16 — `crate::meta` alias artifacts: 6 files** (directionally legal — `crate::meta` = `pub use repo::meta`, `lib.rs:29`): `table/support/model/buffer_config.rs`, `tx/replication/follower_durability/receipt/stage.rs`, `repo/meta/{recovery_marker.rs, repl/bookmark_v3.rs, repl/receipt.rs, repo_identity.rs}`.

### 2.4 Legal downward edges that constrain the target order

instance→repo 15 · tx→repo-low 12 · table→repo-low 16 · query→table 10 (`TableManager`, `record_cow::RecordCow`) · query→repo 5 · instance→table 10 · instance→tx 9 (`CommitError`, `CommitTailBarrier`, `ReplicatedEvent`, `TxOutcome`, follower-durability receipt types, clean-horizon checkpoint/retention gate, cold-reconcile lifecycle, `Drainer`, `recover_inflight_v2`, `commit_tx_with_options`) · tx→table 9 (`WriterDrainGuard` ×10 ref-occurrences — `WriterDrainBarrier` lives in `table/support/guards/writer_drain_barrier.rs` with its loom model in `table/support/guards/tests/loom_model.rs`; `table_token_for` ×3; `RecordCounter` ×1) · validator→table 1 (`TableManager`, `RecordCow` in `validator_db.rs`) · db_instance→repo 2 · db_instance→table 1 · db_instance→repo/instance 1.

## 3. Target layer order

**Final in-crate order: `repo < table < tx < query(batch) < repo/instance < db_instance`.**

The brief's candidate order measured the graph **with re-export shadows in place**. Two corrections follow once shadows are discounted:

1. **Swap `query < tx` → `tx < query(batch)`.** The candidate order's evidence for putting query below tx was the single `tx → query` edge, `tx/preparation/predicate_range.rs` — which is a **shadow artifact**: it imports leaf-crate items (`Filter`, `FilterValue`, `intern_field_path`) through the in-crate `crate::query` re-export and dissolves by importing `shamir_engine_query` directly (cross-crate, order-irrelevant). Meanwhile the genuine flow is the opposite: query/batch **commits through tx** (pair 6: 3 files using `CommitError`, `ack::commit_tx_with_kind`, `TxOutcome`, `release_pessimistic_locks`), and the instance layer genuinely drives tx (pair 1: 35 files). `tx` must therefore sit **below** `query(batch)` so `batch → tx` is downward.
2. **`validator` ceases to be a layer.** The portable validators are already the separate crate `shamir-engine-validator` (below `table`); the in-crate remainder is only `engine_bound/` (807/2,003 lines), which is table-adjacent glue and **folds into `table`** (§5.5). At adoption time `validator` is still expressible as a temporary layer (§8) so the fold lands as a ratchet-to-zero.

`query` as a layer means only `query/batch/` + the facade `mod.rs`; the leaf content lives in the `shamir-engine-query` crate below `table`.

## 4. Target crate layout

| crate | owns | engine-internal deps | prod/test lines |
|---|---|---|---|
| `shamir-engine-repo` (NEW, "repo-low") | `repo/` minus `instance/` minus seed-install orchestration: journal backend (group commit, durability policy incl. `commit_policy.rs`/`recovered_open.rs`/`replication_page.rs` after the handle-parameter fix), `changelog_store` + `write_domain` (`RepoWriteDomain`/`RepoWritePermit`/`RepoWriteReservation`), `repo_types` (`BoxRepo`/`BoxRepoFactory`/`RepoFactory`), `meta/`, config, `version_provider` (moved from `repo/instance/version_provider.rs` — pure token utility, no root refs), the new `DurabilityHandle` bundle (§6), pure seed-install types (`receipt.rs`, `row.rs`, `SeedInstallState`) | none (shamir-storage/shamir-wal/shamir-tx/shamir-types/shamir-collections/shamir-index only) | ~4.1k / ~10.0k |
| `shamir-engine-table` (NEW) | `table/` as-is + folded engine-bound glue (`table/validator_bound/`) + `table/catalog/` (`TableCatalog`, §6) + the table-side `TableResolver` trait + `observe_gate_with_budget` + `table/migration/` (coordinator, shadow log/key, `MigrationState` — moved down from `db_instance::migration`, §5.15) + `new_table_store_id`/`validate_table_store_name` (§5.10) | `shamir-engine-repo`, `shamir-engine-query` (direct, replacing re-export paths), `shamir-engine-validator` (direct, replacing star-re-export usage), `shamir-query-types`, `shamir-index`, `shamir-tx` (WriterDrainGuard gate types, observe-gate gate types) | 23.3k / 50.2k |
| `shamir-engine-tx` (NEW) | `tx/` as-is + `TxHorizon` + `RepoCore` bundles (§6) | `shamir-engine-repo` (DurabilityHandle, write_domain, seed-install types), `shamir-engine-table` (TableCatalog, TableManager, WriterDrainGuard, `table_token_for`), `shamir-engine-query` (direct, `predicate_range.rs`), `shamir-tx`, `shamir-wal` | 15.6k / 36.9k |
| `shamir-engine` (facade, KEPT) | `query/batch/` + `repo/instance/` (shell + orchestrating impls) + `db_instance/` + `lib.rs` facade re-exports + `[[bench]]` + `examples/` + `tests/` | repo, table, tx, query, validator | 16.7k / 30.5k |
| `shamir-engine-query` | unchanged (leaf) | — | unchanged |
| `shamir-engine-validator` | unchanged (portable validators) | — | unchanged |
| `shamir-db` | unchanged (facade consumer) | — | unchanged |

Engine group lands at exactly **7** crates (cap 7; today 4: `shamir-engine`, `shamir-engine-query`, `shamir-engine-validator`, `shamir-db`). The composition root stays in the facade; dependants keep package names and `-p` flags.

### 4.1 Payoff analysis

- **`table` pays most.** Highest churn (281 files since 2026-09-01) and the largest test suite (50.2k). After the cut, a table-only change recompiles table (23.3k) + facade (16.7k) = 40.0k lines instead of the whole 61.0k lib, and reruns table (50.2k) + facade (30.5k) = 80.7k tests instead of 129.6k (−38%).
- **`tx` pays.** Second-highest churn (231 files). A tx-only change reruns tx (36.9k) + facade (30.5k) = 67.4k tests vs 129.6k (−48%). Independently, the knot MUST be broken anyway for the in-crate layer gate to reach zero, so the extraction is incremental work on a mandatory refactor.
- **`repo-low` pays modestly but is the enabler:** `table` and `tx` must sit on a crate that does not depend on them. Caveat: repo-low churn drags the whole closure (its dependents' tests rerun), so its surface must stay small and stable.
- **batch-as-own-crate does NOT pay — rejected cut.** Batch sits one level under the facade. Measured recompile volume for a batch-only change is identical (10.2k batch + 6.5k rest split vs 16.7k combined facade) and reruns are identical (28.2k + 2.2k vs 30.5k facade tests, within rounding). Batch therefore folds into the facade.
- **The validator fold is not a new crate** — it is a module move that (together with the R1 import rewrites) zeroes the six R1 acceptance pairs (§7.1).

**Explicitly rejected cuts:** batch extraction; a separate engine-bound-validator crate (fold instead); any extraction of `db_instance` (1.4k lines, top layer, stays in facade); moving benches/examples/integration tests out of the facade (they exercise facade APIs).

## 5. Breaking every backward edge

One subsection per pair. Mechanisms tagged with the wave that owns them: [R1] / [R2]/[R2a-d] / [R3] / [R4].

### 5.1 tx → repo/instance (35 files) — [R2c, completed by R4]

The `RepoInstance` knot. Broken by the bundle split (§6): all 35 files rewrite `repo.x()` to `core.x()`/bundle calls against `&RepoCore` (or individual bundles) — a mechanical sweep, no logic change. `resume/lookup.rs`'s unused `_repo: &RepoInstance` parameter is dropped outright. Acceptance: pair 35→0 at end of R2 (cross-crate-legal at R4).

### 5.2 tx → repo-low (12 files) — [R2b], no break needed

Downward-legal under the corrected order. The only change is R2b's handle-parameter fix: `commit_policy.rs`/`recovered_open.rs`/`replication_page.rs` switch from `&RepoInstance` to `&DurabilityHandle` (+ `RepoName`) (§6). At R4 the pair becomes the `shamir-engine-tx → shamir-engine-repo` crate dependency. Note: `tail.rs`'s `visibility_observer` reference is an instance item and is handled under §5.8/§5.10, not here.

### 5.3 table → query leaf (19 files) — [R1]

Rewrite `use crate::query::...` to `use shamir_engine_query::...`. Items are identical: `FilterNode`, `FilterContext`, `compile_filter`, `intern_field_path`, `query::read` types (incl. `read::order::PREALLOC_CAP`), `query::write` types. Zero behavior change; the crate is already a dependency.

### 5.4 table → query batch (5 files) — [R1]

- `RowByteMeter::value_size` (pure helper measuring `QueryRecord` bytes; defined in `query/batch/exec/exec_scope.rs`, also used by `query/batch/runner/query_runner/arms_read.rs`): move into `shamir-engine-query`'s read module (the leaf owns `QueryRecord`) and switch `exec_scope.rs`/`arms_read.rs` to import it from the leaf — batch→leaf is legal downward.
- `crate::query::TableResolver` in the 4 write exec files + `collect.rs`: split the trait (§5.4.1).

**5.4.1 TableResolver split.** `executor_traits.rs` currently defines one trait with `resolve_repo(&self, repo_name) -> DbResult<crate::repo::RepoInstance>`. Split:
- NEW **table-side trait** in `table/support/model/table_resolver.rs` (file name is this design's suggestion; the wave owner may adjust within `table/support/model/`) holding the table-scoped resolution used by the write paths and `validator_db.rs` — `resolve_table -> Arc<TableManager>` / schema+config lookup; exact method set mirrors what the six current callers use (4 write exec files, `collect.rs`, `validator_db.rs` + batch executors via `executor_traits`).
- `executor_traits.rs` KEEPS the repo-scoped resolution: keep the path `crate::query::batch::executor_traits::TableResolver` resolving by re-exporting the table-side trait, and carry the repo-scoped method on a separate port; after R2d `resolve_repo` returns `Arc<RepoCore>` (tx-crate type, legal for batch) instead of `RepoInstance`.
- `query/mod.rs:443` keeps `pub use ... TableResolver` alive (source switched to the table-side trait) so `shamir_engine::query::TableResolver` keeps resolving for dependants.
- Dispatch shape must not change: if callers hold the trait generically today, keep generics; no new `dyn` on write/read hot paths.

### 5.5 table → validator (9 files) + engine_bound fold — [R1]

- Portable half: the 9 files import `shamir_engine_validator::...` directly for `WriteOp`, `ValidatorRegistry`, `ValidatorBinding`, `ValidatorFailure`, `RecordValidator`, `RecordFields`, `ForeignKeyRef`, `TransformSpec`.
- Glue half: fold `validator/engine_bound/{mod.rs, persistence.rs, validator_db.rs}` → `table/validator_bound/` (new module under `table/`). `validator/mod.rs` keeps the public paths `shamir_engine::validator::{ValidatorDb, ValidatorFkCtx, BatchFkCache, FkCacheEntry, persistence}` by re-exporting from the new home: before extraction `pub use crate::table::validator_bound::validator_db::{...}`; after R4 `pub use shamir_engine_table::validator_bound::...`. `validator/mod.rs` continues `pub use shamir_engine_validator::validator::*` unchanged.
- `validator_db.rs` switches `TableRef` to `shamir_query_types::TableRef` direct and `TableResolver` to the table-side trait (table-internal after the fold).
- `engine_bound/` has a `tests/` dir (verified: `computed_default_transform_tests.rs`, `fk_probe/{fk_batch_cache_tests,staged_probe_tests}.rs`, `persistence_tests.rs`, `run_validators_tests.rs`, `validator_actor_tests.rs`, `validator_db_tests.rs`) — it moves with the fold under `table/validator_bound/tests/` and is declared in the campaign's `discovery_delta`.

### 5.6 query/batch → tx (3 files) — [R2d ports, cross-crate-legal at R4]

The batch layer defines a narrow commit port — suggested name `TxCommitSink`, or a slightly wider `BatchTxPort` holding commit + durability flush + lock release; exact name is the wave owner's choice. Single method(s) commit and return tx-crate types (`TxOutcome`/`CommitError` are legal for batch once tx is a crate). Implementations live in the instance layer (impl blocks on `RepoInstance`-owned adapters or a small adapter in `repo/instance/`) and just forward to `tx::ack` + repo methods.

- `receipts/commit.rs`: replace the direct `commit_tx_with_kind(tx, repo, wait, PendingKind::ImplicitBatch)` call with the port. Its `receipt_error_code` (pure `CommitError`→wire mapping) **stays in batch**, importing tx types — legal post-extraction.
- `batch_execute.rs`: `synced_flush`/`sync_wal`/`commit_visibility_wait`/`commit_tx_with_options`/`release_pessimistic_locks` go through `BatchTxPort`; note `release_pessimistic_locks` is a tx free fn taking `(&tx, &repo)` — the port impl closes over the repo handle on the instance side.
- `interactive_tx.rs`: `begin_tx`/`commit_tx` through the port.
- BEFORE R4 the in-crate query→tx edge remains at 3 (the acceptance table shows query→tx reaching zero **at R4**, not R2).

### 5.7 query/batch → repo/instance (3 files) — [R2d]

- `executor_traits.rs`: `resolve_repo` returns `Arc<RepoCore>` (§6).
- `receipts/commit.rs`, `interactive_tx.rs`: the port impls above remove the `&RepoInstance` holds; acceptance: query→instance 3→0, attributed to R2d.

### 5.8 repo/instance → query (2 files + 1 doc link) — [R2a] + [R2c]

- `fk_reverse_cache.rs` [R2a]: `crate::query::batch::TableResolver` → table-side trait (§5.4.1); `crate::query::TableRef` → `shamir_query_types::TableRef` direct. Its query refs disappear; instance→query-batch 2→1.
- `transactions.rs` [R2c]: contains four implicit-batch convenience methods — `run_implicit_batch_tx` (:410, HRTB `stage` closure, maps stage errors to `BatchError::QueryError{alias, message, code}` at :425), `begin_implicit_batch_tx` (:445), `begin_implicit_batch_tx_with_isolation` (:473, maps `begin_tx` errors to `BatchError::QueryError` at :482), `commit_implicit_batch_tx` (:500, delegates to `crate::query::batch::receipts::commit_implicit(self, tx, alias, VisibilityWait::Strict)` at :505) — plus `use crate::query::write::WriteResult` (:3, leaf shadow). Mechanism: move the method bodies into `query/batch/implicit.rs` as free fns over `&RepoInstance` (same shape as `receipts::commit_implicit`); `WriteResult` switches to `shamir_engine_query::write::WriteResult` direct (kills the leaf shadow). See Appendix A.3 for why the shell keeps thin delegating methods (public-API evidence).
- `repo_instance.rs:39` doc link [`crate::query::batch::Authorized`] rewritten to the facade path in the R2c pass.

### 5.9 repo(seed) → table (1 file) — [R3]

`inventory.rs` moves up with the seed-install orchestration (§5.14); its `crate::table::TableConfig` import becomes instance→table (legal).

### 5.10 table → repo/instance (2 files) — [R3]

- `table_config.rs`: move `new_table_store_id`/`validate_table_store_name` (pure fns, `repo/instance/repo_instance/table_store/names.rs`, with `tests/names.rs`) DOWN into `table/support/model/`; keep the `repo_instance::table_store` path resolving via a re-export in `table_store/mod.rs`.
- `read_temporal.rs`: move `observe_gate_with_budget` (pure fn over shamir-tx gate types + budget) DOWN into `table` (suggested `table/read/planning/` or `table/support/`); keep `crate::repo::visibility_observer` alive via the `repo/mod.rs:13` re-export switched to the new home; `tail.rs`'s import of it then reads table-side (tx→table legal).

### 5.11 table → tx (1 file) — [R3]

`ddl_barriers.rs`: define a table-local `MigrationFreezeError { Storage(DbError), MigrationFrozen{...same fields as CommitError::MigrationFrozen} }`; `begin_migration_freeze`/`begin_migration_freeze_admitted` return it; the single in-crate caller `repo/instance/repo_instance/catalog/rename_freeze.rs:26` maps it into its own flow. **Verification result (Appendix A.1): there IS one facade-external caller** — `crates/engine/shamir-db/src/shamir_db/execute/admin_db/admin_migration.rs:151` calls `begin_migration_freeze_admitted(&src_permit)` — so the facade/instance keeps a thin public wrapper returning `CommitError` (facade→tx legal), and table-internal tests (`table/tests/index_lifecycle/registry/migration_freeze_tests.rs`, `table/tests/admission/domain/write_domain_admission_matrix_tests.rs`) stay with `table` and switch to the table-local error.

### 5.12 tx → query leaf (1 file) — [R2c, sweep chunk]

`predicate_range.rs`: rewrite to `use shamir_engine_query::...`. Done inside the sweep chunk that owns `tx/preparation/`.

### 5.13 validator(engine_bound) → query (1 file) — [R1]

Covered by the fold (§5.5): `validator_db.rs` switches `TableRef` to `shamir_query_types::TableRef` direct and `TableResolver` to the table-side trait.

### 5.14 repo-low → repo/instance (5 files) — [R2b] + [R3]

- Durability-policy files (`commit_policy.rs`, `recovered_open.rs`, `replication_page.rs`) [R2b]: take `&DurabilityHandle` (+ `RepoName`) instead of `&RepoInstance` (§6).
- Seed-install orchestration [R3]: `repo/seed/install/{inventory.rs, writer.rs}` and `repo/seed/prepare_seed.rs` relocate under `repo/instance/seed/`. `SeedImageWriter` legitimately holds the root (it orchestrates `add_table_admitted`/flushes; verified both root-touching files call only shell methods — `writer.rs:300` and `prepare_seed.rs:80` call `repo.flush_seed_metadata()` — so R3 has **no** dependency on R2b's handle). Pure seed-install types (`install/receipt.rs`, `install/row.rs`, `SeedInstallState`) stay in repo-low (tx depends on them); the `install/tests/` dir splits with its subjects (declared in `discovery_delta`). `repo/mod.rs` seed re-exports (`seed_install`, `prepare_seed`, `SeedImageWriter`, ...) rewire: type paths stay on repo-low, orchestration paths point at the instance module.

### 5.15 table → db_instance via `crate::migration` (1 file) — [R3]

**Verification result (Appendix A.2):** `admitted_ops.rs` does not merely import `MigrationCoordinator` — it **constructs** it (`MigrationCoordinator::new` at :267 and :306, with `.with_migration_freeze(...)`/`.with_domains(...)` builders; both fns return `Arc<MigrationCoordinator>`), and `read_only_views.rs:175` calls `MigrationCoordinator::load_state`. `MigrationState` is defined in `db_instance/migration/coordinator.rs:39` (not in the shadow files), and `coordinator.rs`'s only in-crate deps are `crate::repo::{RepoWriteDomain, RepoWritePermit}` and `crate::table::MigrationFreezeGuard` — both legal in `table`. Therefore the **whole `db_instance/migration/` module moves down into `table/migration/`** (coordinator.rs, shadow_log.rs, shadow_key.rs, mod.rs, `tests/{coordinator_tests,cutover_guard_tests,shadow_key_tests,shadow_log_tests}.rs`); `MigrationCoordinator` does **not** stay in `db_instance`. `db_instance` keeps `pub use crate::table::migration;` and the facade keeps `shamir_engine::migration` (`lib.rs:27`) plus `shamir_engine::db_instance::migration::*` resolving — external users (`crates/engine/shamir-db/src/shamir_db/execute/admin_db/admin_migration.rs:6` for `MigrationPhase`/`MigrationState`, `crates/engine/shamir-db/tests/rename_migration/migrations/migration_index.rs:20` for `MigrationCoordinator`/`MigrationShadowLog`) keep their paths. `admitted_ops.rs` and `read_only_views.rs` switch `crate::migration::` to the in-crate `crate::table::migration::` (same-crate after the move). `read_only_views.rs` is also an R1 file — sequencing constraint in §7.3 (file set 4) and Appendix A.5.

### 5.16 `crate::meta` alias artifacts (6 files) — [R3]

`buffer_config.rs`, `stage.rs`, and the four `repo/meta/` files switch `crate::meta::` to canonical paths (`crate::repo::meta::` pre-extraction; `shamir_engine_repo::meta::` post-R4). Directionally legal today; pure canonicalization.

## 6. The RepoInstance knot

`RepoInstance` (`repo/instance/repo_instance.rs:41-174`) is the composition root: every field is an `Arc`, clones are shallow struct clones, tx receives `&RepoInstance` sibling params (e.g. `commit/acknowledgement/tail.rs:33-35`) or moves clones into detached tasks (`commit_driver.rs:35-39`). Strategy: **split the type into three Arc-field bundles + one aggregate, all plain structs (static dispatch everywhere, zero new `dyn` on hot paths), each owned by its lower layer, plus a thin shell.**

### 6.1 `TableCatalog` (table layer; new module `table/catalog/`)

Fields: `configs: Arc<TDashMap<String, TableConfig>>`, `tables: Arc<TDashMap<String, Arc<OnceCell<TableManager>>>>`, `token_names`, `per_table_mvcc`, `rename_fence` (+`rename_fence_len`), `repo_interner: Arc<OnceCell<InternerManager>>`. Impls MOVE from `repo/instance/repo_instance/catalog.rs` + `catalog/repo_membership_impl.rs`: `table_by_token{,_if_live}` (catalog.rs:158/206), `get_table{,_admitted,_unfenced}` (catalog/repo_membership_impl.rs:82-107), `has_table`, `list_table_names`, `mapped_table_store_names`, `table_count` (catalog.rs:268-293). `RepoInstance` keeps its PUBLIC methods as one-line delegating wrappers (impl blocks stay in `repo/instance` — the `owned_impl.toml` entry remains valid).

### 6.2 `DurabilityHandle` (repo-low; journal backend module)

Fields: `repo_wal` cell, `wal_group` cell, `group_commit`, `changefeed` cell, `pending_changefeed_gap`, `durability: Arc<ArcSwap<DurabilityPolicy>>`, `durability_metrics`, `wal_retention_gate`, `write_domain`, `marker_write_mutex`, `wal_dir`. Impls MOVE from `lifecycle/journal.rs` (`prepare_journal_event`, mark/record pending changefeed gap, `broadcast_changefeed_event`, bounded read), `lifecycle/durability_repo_impl.rs` (repo_wal/wal_group/durability accessors, `settle_unwritten_version`, `note_unclean_open_bump`), `lifecycle/clean_horizon_repo_impl.rs` (`wal_retention_gate` accessor, `wal_retention_refuse_writes`, `reserve_recovery_floor`), plus `follower_journal_store`/`changelog_store_for_cold_reconcile` from `transactions/follower_durability.rs:10` and `cold_reconcile_repo_impl.rs:7`. Lazy `OnceCell` semantics are preserved by CLONING the `Arc<OnceCell<...>>` into the handle (init stays where it is; no eager-init behavior change).

### 6.3 `TxHorizon` (tx layer)

Fields: `tx_gate` cell, `tx_metrics`, `commit_tail`, `drainer` cell, `commit_visibility_wait`. Impls MOVE from `transactions.rs` (`tx_gate`/`initialized_tx_gate`/`commit_visibility_wait`), `lifecycle.rs` (`commit_tail`/`drainer`/`tx_metrics` accessors), `catalog/visibility_impl.rs` (`visible_through_sample`). The drainer's `OnceLock` init spawns a background clone — that init moves to the SHELL: `TxHorizon` exposes `drainer_cell()` and the spawn-once closure is installed by `RepoInstance` at shell construction (it has the root), so tx-side callers just read the cell. `get_or_init` stays single-init.

### 6.4 `RepoCore` (tx layer; suggested module `tx/core.rs` — one file, one primary export)

The aggregate: `name: RepoName` (or `String`), plus `Arc<TableCatalog>` + `Arc<DurabilityHandle>` + `Arc<TxHorizon>` + `worker_lifecycle` + `receipt_worker` + `fk_reverse_cache` + `replication_armed` + `wal_segment_bytes` + `wal_dir` reference + `cfg(test)` seam fields that tx needs. CONSTRUCTED by `RepoInstance` (which owns everything) and passed DOWN instead of `&RepoInstance`. tx fns switch signatures from `repo: &RepoInstance` to `core: &RepoCore` — or to the individual bundles where a fn needs only one (e.g. `preparation/*` take `&TableCatalog` + `Arc<TxMetrics>`).

### 6.5 `RepoInstance` shell (facade, `repo/instance/`)

Fields: `name`, `live`, `core: Arc<RepoCore>` (+ `cfg(test)` seams the shell itself needs). Public API methods stay: `begin_tx`, `commit_tx`, `commit_tx_with_kind/options` (delegating into tx entry points now taking `&RepoCore`), `add_table`, seed orchestration, changefeed public accessor, `subscribe_changelog`, etc.; plus `from_factory`/`from_box_repo` constructors and `clone_for_background` (`live = None`). All 35 tx files rewrite `repo.x()` → `core.x()`/bundle calls — mechanical, no logic change.

### 6.6 Hot paths stay static

CommitDriver's `Make: FnOnce(TxContext, RepoInstance)` becomes `FnOnce(TxContext, RepoCore)` (`commit_driver.rs:35-39`); `resume/worker.rs` stores `RepoCore`; the seed writer keeps the shell (it orchestrates the public API). Everything on the commit/preparation/materialize/follower-publish paths must stay plain structs of `Arc`s — no new dyn traits. Cold paths (recovery, retention, seed, receipt resume, truncate) tolerate `dyn` but static is preferred uniformly; note dynamic dispatch is ALREADY present on the commit path via `tx_info_store` (`lifecycle.rs:224`, returns `Arc<dyn Store>`).

### 6.7 `owned_impl.toml` fate and rust-guard interplay

The `[[owned_impl]] type = "RepoInstance" module = "crate::repo::instance::repo_instance"` entry STAYS and remains correct — the shell's impl blocks remain in `repo/instance`. The MOVED impl blocks become inherent impls of `TableCatalog`/`DurabilityHandle`/`TxHorizon`/`RepoCore` in their own defining modules — no `owned_impl` entry needed (an inherent impl in its own module is the default-legal case). Trait impls (`BatchTxPort` etc.) are not governed by the gate (the gate covers inherent+trait impls of CONFIGURED types only).

### 6.8 Test seams

The `cfg(test)` seam fields (`journal_publish_hook`, `table_init_hook`, `clean_horizon_test_state`, `background_drainer_disabled`) move with `RepoCore`, but the `cfg(test)` module reference `tx::tests::journal_publish_order_tests` must keep resolving — while both are in-crate the instance cfg(test) field may keep the tx path; at R4 this must be revisited (listed as a known R4 check in §9).

## 7. Refactoring waves

### 7.0 rust-guard campaign usage (applies to every wave)

Per wave:
- Add a `[[campaigns]]` entry to `.rust-guard/stages.toml` (`packages = ["shamir-engine"]` for R1–R3; add the new packages at R4).
- Add a delta file `.rust-guard/stages/<wave>.toml` with `base = "<merge-base sha>"` and `[[moved]]` entries (`path`/`item`/`to_path`/`to_item`, `reason`, `owner`) for every relocated fn/struct; `[[changed]]` where signatures change (the `&RepoInstance` → `&RepoCore` sweeps change signatures en masse — declare the per-file item entries); `[[allow_widen]]` for items that must become `pub` to cross the future crate boundary (cross-crate visibility is a widening vs merge base). Owner values are wave placeholders (`owner = "<wave-R1>"`), never person names.
- Tests that move with their modules are declared in the campaign's `discovery_delta` (same file shape). Test losses are NEVER declarable in the conservation delta — they must be real `[[moved]]` entries with `to_path`/`to_item` naming the moved test binary/test.
- `--suggest-delta` produces a first draft of moved entries to review.
- Landed campaign entries are removed by the next wave's PR (inert once landed).
- The structure gate's VIS_WIDENING/ALLOW_ADDED rules apply throughout — no new `#[allow]`; pub-cranewidth goes through the delta file.

Shapes:

```toml
# .rust-guard/stages.toml (fragment; entry removed once the wave lands)
[[campaigns]]
name = "engine-layering-R1"
packages = ["shamir-engine"]
delta = "stages/engine-layering-R1.toml"
```

```toml
# .rust-guard/stages/engine-layering-R1.toml (illustrative fragment)
base = "<merge-base sha>"

[[moved]]
path = "src/validator/engine_bound/validator_db.rs"
item = "ValidatorDb"
to_path = "src/table/validator_bound/validator_db.rs"
to_item = "ValidatorDb"
reason = "R1: engine_bound folds into table/validator_bound"
owner = "<wave-R1>"

[[moved]]
path = "src/query/batch/exec/exec_scope.rs"
item = "RowByteMeter::value_size"
to_path = "<shamir-engine-query>/src/query/read/row_byte_meter.rs"
to_item = "RowByteMeter::value_size"
reason = "R1: pure QueryRecord-byte helper moves to the leaf that owns QueryRecord"
owner = "<wave-R1>"

[[changed]]
path = "src/table/write/insert.rs"
item = "insert_row"
reason = "R1: crate::query::* -> shamir_engine_query::* (re-export shadow dissolved)"
owner = "<wave-R1>"

[[allow_widen]]
path = "src/table/support/model/table_resolver.rs"
item = "TableResolver"
reason = "pub across the future shamir-engine-table boundary (R4)"
owner = "<wave-R1>"
```

### 7.1 R1 — "table faces outward"

One agent, one worktree; parallel with R2 (which starts only after R1 merges) and R3.

**File set:** the 19 leaf-shadow table files (§5.3 list) + the 9 validator-edge table files (overlap: add `table_manager.rs`, `table_manager_validators.rs`, `write_domain_barrier.rs`) + NEW table-side `TableResolver` trait (`table/support/model/table_resolver.rs`) + NEW leaf home of `RowByteMeter::value_size` (in `shamir-engine-query` read module) + `exec_scope.rs`/`arms_read.rs` import switch + engine_bound fold (`validator/engine_bound/{mod.rs,persistence.rs,validator_db.rs}` + `tests/` → `table/validator_bound/`; `validator_db.rs` gets the `TableRef`+`TableResolver` switch as part of the move) + `validator/mod.rs` re-export rewiring + visibility-observer move (partial: `observe_gate_with_budget` out of `visibility_impl.rs` to its new table home; `read_temporal.rs` switches) + `query/mod.rs` `TableResolver` re-export source switch.

**Order constraint:** R1 merges before R2 starts (`visibility_impl.rs` — also needed by R2c for `visible_through_sample` — and the table-side `TableResolver` are R2 prerequisites; R2c additionally requires R1's completed `visibility_impl.rs`).

**Acceptance (pairs reaching zero in the layers baseline):** table→query-leaf 19→0; table→query-batch 5→0; table→validator 9→0; validator→table 1→0; validator→query-batch 1→0; validator→query-leaf 1→0; table→instance 2→1 (`table_config.rs` remains, R3).

**Risks:** public-path preservation for `shamir_engine::validator::*` and `shamir_engine::query::TableResolver`; the `RowByteMeter` move touches `shamir-engine-query`'s public read module; the `TableResolver` split must keep every current caller compiling (6 callers: 4 write exec files, `collect.rs`, `validator_db.rs`, + batch executors via `executor_traits`).

### 7.2 R2 — "dissolve the RepoInstance knot"

One agent, sequential sub-waves R2a→R2b→R2c→R2d in ONE worktree; must not start before R1 merges (`visibility_impl.rs` and the table-side `TableResolver` are prerequisites). `repo_instance.rs` and the lifecycle files are touched by MULTIPLE sub-waves — fine WITHIN one agent's sequential R2; the non-overlap guarantee is BETWEEN R1/R2/R3.

**R2a — catalog:** `repo_instance.rs` (field swap), `catalog.rs`, `catalog/repo_membership_impl.rs` (impls move to `TableCatalog`), `support/catalog/fk_reverse_cache.rs` (switch to table-side `TableResolver`; `TableRef` → direct `shamir_query_types` import; its query refs disappear), NEW `table/catalog/` module files. Acceptance: instance→query-batch 2→1.

**R2b — durability:** `repo_instance.rs`, `lifecycle/journal.rs`, `lifecycle/durability_repo_impl.rs`, `lifecycle/clean_horizon_repo_impl.rs` (retention parts), NEW `DurabilityHandle` files in the repo-low journal backend, repo-low `commit_policy.rs`/`recovered_open.rs`/`replication_page.rs` (handle params). `transactions.rs` stays OUT of R2b (goes to R2c).

**R2c — horizon + core + sweep:** `repo_instance.rs` (shell + `RepoCore` construction + `clone_for_background` + drainer install), `transactions.rs` (tx_gate/commit_visibility_wait accessors → `TxHorizon`; implicit-batch bodies → `query/batch/implicit.rs` with shell delegates kept — Appendix A.3; `WriteResult` leaf shadow → direct import; this leaves instance→query-batch at the documented residual 1, Appendix A.3), `lifecycle.rs` (`commit_tail`/`drainer`/`tx_metrics` accessor move), `catalog/visibility_impl.rs` (`visible_through_sample` → `TxHorizon`; completes the file after R1), THEN the 35-file tx caller sweep in three chunks: `commit/` (8 files), `preparation/` (6) + `drain/` (5), `recovery/` (7) + `replication/` (9). `predicate_range.rs`'s leaf-import rewrite happens in its sweep chunk.

**R2d — batch ports:** `executor_traits.rs` (`resolve_repo` → `Arc<RepoCore>`), `interactive_tx.rs`, `batch_execute.rs`, `receipts/commit.rs` (`BatchTxPort`), `query/mod.rs` (TableResolver re-export path — source switched in R1; R2d only confirms it still resolves once `resolve_repo` returns `Arc<RepoCore>`). Acceptance attributed to R2d: query→instance 3→0.

**Acceptance R2 overall:** tx→instance 35→0; query→instance 3→0 (the three files attributed to R2d); instance→query-batch 2→1 residual (Appendix A.3 — design target was 2→0); instance→query-leaf 2→0; tx→query-leaf 1→0. (query→tx stays 3 until R4 by design.)

**Risks:** signature-change avalanche (mitigated by conservation `[[changed]]` entries + `--suggest-delta`); lazy-init semantics (clone cells, never eager-init); the drainer spawn-once invariant (`OnceLock` `get_or_init` stays single-init, now installed at shell construction); hot-path dispatch must remain static; `cfg(test)` seam move with `RepoCore` while `tx::tests::journal_publish_order_tests` keeps resolving (known R4 check).

### 7.3 R3 — "small edges"

One agent; parallel with R1/R2 — only file-set overlap constraints apply. File sets:

1. Seed move up: `repo/seed/install/inventory.rs`, `repo/seed/install/writer.rs`, `repo/seed/prepare_seed.rs` → `repo/instance/seed/`; `repo/seed` remnants (`install/mod.rs`, `install/receipt.rs`, `install/row.rs`, `install/tests/`) + `repo/mod.rs` seed re-exports rewired (§5.14). Verified: no R2b dependency (§5.14).
2. Store-name fns down: `table/support/model/table_config.rs` + `repo/instance/repo_instance/table_store/{names.rs, tests/names.rs, mod.rs}` (§5.10).
3. Meta canonical imports: `table/support/model/buffer_config.rs`, `tx/replication/follower_durability/receipt/stage.rs`, `repo/meta/{recovery_marker.rs, repl/bookmark_v3.rs, repl/receipt.rs, repo_identity.rs}`.
4. Migration module down: `table/table_manager/admission/domain/barrier/admitted_ops.rs` + `table/table_manager/admission/views/read_only_views.rs` (import switch only) + the whole `db_instance/migration/` tree → `table/migration/` (§5.15). **Constraint:** `read_only_views.rs` is also an R1 file — the migration chunk of R3 must land after R1 (Appendix A.5).
5. Freeze-error split: `table/table_manager/barriers/ddl_barriers.rs` + `repo/instance/repo_instance/catalog/rename_freeze.rs` + the public wrapper for `shamir-db` (§5.11).

**Acceptance:** repo→table 1→0; repo→instance 5→0 (seed files up; durability-policy files already fixed in R2b); table→instance 1→0 (`table_config.rs` fn move); table→tx 1→0; table→db_instance 1→0 (migration alias); alias artifacts →0.

### 7.4 R4 — "crate extraction"

Sequential, after R1–R3 merged; one agent.

- Create the three `Cargo.toml`s; move module trees WITH their `tests/` dirs.
- Facade `lib.rs` becomes pure re-exports. Concretely (respecting the repo rule that mod.rs files are re-export-only):

```rust
pub use shamir_engine_repo as repo_low;            // illustrative name; final names fixed at R4
pub mod repo {
    pub use shamir_engine_repo::*;
    pub use shamir_engine_repo as repo_low_inner;  // shape only — spell exact wiring in R4
    pub mod instance;                              // stays in-facade source
}
```

The exact wiring must preserve every public path enumerated in §9.1 (`engine::repo::*` keeps resolving through a `pub mod repo` shim that re-exports the repo-low crate plus the in-facade `instance` subtree).
- Feature forwarding: `default = ["all-backends"]`, `all-backends = ["fjall"]`, `fjall → shamir-engine-repo/fjall` (BoxRepo variants live there) `+ shamir-engine-table/fjall` as needed; `test-util → subcrates' test-util` (mirrors today's forwarding to engine-query/engine-validator); `loom → shamir-engine-table/loom` + `dep:loom` stays where the loom module lives; `fts_indexed_long` stays on the facade for its bench.
- `build.rs` loom-cfg emission moves to the crate owning the loom module (`shamir-engine-table`).
- `[[bench]]` table stays on the facade (benches exercise facade APIs; `required-features` strings keep working via forwarded features — VERIFY each bench's API usage stays on the facade surface during R4 and note any that must move).
- Integration tests (`tests/` dir) stay on the facade.
- Doctest gate: the `TableManager` boundary compile-fail example moves with `TableManager` → the `[lib]` doctest enablement and the CI doctest `-p` flag move to `shamir-engine-table` (or a second doctest-enabled crate) — called out explicitly here and in §9.
- CI updates: `rustdoc` job gains the three new crates (mirror the query/validator pattern: default + `--no-default-features` for repo/table/tx at minimum; `test-util` variants where the feature exists); `engine-features` EXPECTED map gains forwarded expectations for the three crates; job `-p` flags update; `loom-model` job re-points to `shamir-engine-table`; `test-discovery` `test-util-fts` flags stay facade-scoped (features forward); `scripts/test.sh` scopes `@engine`/`@oracle` gain the new packages; stress-nightly `@oracle` unchanged semantically.
- `scripts/ci/matrix/edges.toml` needs NO new entry: the matrix maps changed paths to the nearest owning package via `cargo metadata` manifest dirs, so the new crates are picked up automatically, and no hidden non-Cargo edge is introduced (edges.toml holds only EXTRA hidden edges — wasm/ts/deploy/lockfile; it has no engine entry today).
- Campaign: stages.toml entry for R4 with cross-package `[[moved]]` entries (`to_path` naming the new packages) + `discovery_delta` for moved tests.

**Acceptance R4:** query→tx 3→0 (cross-crate); the monolith's remaining baseline is empty; the facade crate's in-crate module set is `query(batch)` + `instance` + `db_instance`, with every upward pair among them baselined at zero (§8).

### 7.5 Landing order

R1 ∥ R3 immediately (R3's migration chunk after R1 — §7.3); R2 after R1; within R2: R2a → R2b → R2c → R2d; R4 last, after R1+R2+R3. Merge-order constraint: R1 → R2c (visibility/`TableResolver` dependencies); R2b before R2c. R3 is file-wise independent of R1/R2 except the migration chunk (A.5) and has no R2b dependency (verified, §5.14).

## 8. The layers.toml gate and content

**Schema ownership caveat:** the `layers.toml` schema has NO existing owner — no `layers` support in `tools/rust-guard` (grep clean), no `.rust-guard/layers.toml`, and the only repo-wide mention is the task brief itself. This document PROPOSES the schema below; the gate implementation is a separate task. The gate enforces only crates that declare `layers`; a file may reference only modules at or below its own entry; `baseline` is the per-pair file-count ratchet (upper→lower violations allowed at adoption; target 0). Counting convention: production files, per §2; re-export shadows counted against their in-crate target module at adoption time.

### 8.1 Adoption baseline (pre-wave; measured 2026-11-14, `3c20d01b8`)

```toml
# .rust-guard/layers.toml — consumed by the future `rust-guard layers` gate (separate task).
# ADOPTION baseline: worktree 3c20d01b8, 2026-11-14. Ratchet; target all-zero.
# `validator` is a temporary adoption-time layer; it dissolves at R1 (fold into table).
[[crate]]
name = "shamir-engine"
layers = ["repo", "table", "validator", "tx", "query", "instance", "db_instance"]

[crate.baseline]
"validator->table" = 1      # validator/engine_bound/validator_db.rs (TableManager, RecordCow)
"validator->query" = 1      # validator_db.rs: TableRef (leaf shadow) + TableResolver (batch)
"table->query" = 24         # 19 leaf-shadow files + 5 batch files
"table->validator" = 9      # portable half dissolves via direct import; glue folds in
"table->tx" = 1             # ddl_barriers.rs (CommitError)
"table->instance" = 2       # table_config.rs, read_temporal.rs
"table->db_instance" = 1    # admitted_ops.rs (crate::migration alias)
"tx->query" = 1             # predicate_range.rs (leaf shadow)
"tx->instance" = 35         # RepoInstance
"query->tx" = 3             # batch commit path
"query->instance" = 3       # receipts/commit.rs, interactive_tx.rs, executor_traits.rs
"repo->table" = 1           # seed/install/inventory.rs
"repo->instance" = 5        # seed writer/prepare + 3 durability_policy files
```

Downward-legal pairs carry NO baseline entry: `tx->repo` (12 files, incl. the `tail.rs` misattribution correction), `table->repo`, `instance->query` (2 files — cleaned in R2a/R2c to a documented residual of 1, Appendix A.3; downward-legal so the gate stays green regardless), `db_instance->*`, and the six `crate::meta` alias files.

### 8.2 Post-R1+R2+R3 (pre-R4) facade baseline

```toml
[[crate]]
name = "shamir-engine"
layers = ["repo", "table", "tx", "query", "instance", "db_instance"]

[crate.baseline]
"query->tx" = 3   # cross-crate-legal only at R4
# every other upward pair: 0 (validator layer dissolved)
```

### 8.3 Final (post-R4)

```toml
# Facade: batch + instance + db_instance remain; all upward pairs zero.
[[crate]]
name = "shamir-engine"
layers = ["query", "instance", "db_instance"]

[crate.baseline]
# empty — no upward pairs remain in the facade

# Single-layer crates declare NO layers entry; the gate enforces only crates that
# declare layers. shamir-engine-repo, shamir-engine-table, shamir-engine-tx have no
# internal layering to enforce:
# [[crate]] name = "shamir-engine-repo"   (no layers)
# [[crate]] name = "shamir-engine-table"  (no layers)
# [[crate]] name = "shamir-engine-tx"     (no layers)
```

`shamir-engine-query` and `shamir-engine-validator` keep their current (layerless) treatment.

## 9. Risks and invariants

### 9.1 Public API (facade re-export table)

Every path the dependants use must keep resolving. Dependant usage of `shamir_engine::` (production+test files, counted by root segment): shamir-db — `function` 89 (re-export of `shamir_wasm_host`, `lib.rs:12`), `repo` 58, `validator` 47, `meta` 19, `table` 11, `tx` 9, `query` 6, `ChangelogEvent` 4, `VisibleOutcome` 3, `JournalRead` 2, `BoundedJournalRead` 1; shamir-server — `query` 1; shamir-server-security — `tx` 3; shamir-sync — `DEFAULT_MAX_TX_LIFETIME` 1. `shamir-db/src/lib.rs` re-exports `pub use shamir_engine as engine; pub use shamir_engine::query;`. Preserved paths: `engine::function`, `engine::repo::*`, `engine::validator::*`, `engine::meta::*`, `engine::table::*`, `engine::tx::*`, `engine::query::*`, `ChangelogEvent`, `VisibleOutcome`, `JournalRead`, `BoundedJournalRead`, `shamir_db::query`. Method-surface callers found during verification (must keep compiling): `RepoInstance::run_implicit_batch_tx` — 7 production call sites in `shamir-db/src/shamir_db/` (`system_store/store.rs:497,524`, `system_store/identities.rs:49`, `system_store/catalogue/rename_recovery.rs:155`, `system_store/catalogue/ops.rs:394`, `execute/repo_admin/quarantine_helpers/admin_replication.rs:353`, `execute/admin_db/admin_replication.rs:818`) plus test/e2e references; `begin_migration_freeze_admitted` — `shamir_db/execute/admin_db/admin_migration.rs:151`; `engine::migration::{MigrationCoordinator, MigrationShadowLog, MigrationPhase, MigrationState}` — `admin_migration.rs:6`, `tests/rename_migration/migrations/migration_index.rs:20`.

### 9.2 Features matrix

`engine-features` cells (6): no-default, fjall, test-util, fjall-test-util, default-plus `[test-util, fts_indexed_long]`, loom; per cell `scripts/ci/gates/features_matrix_check.py --expected <id>` then `cargo check`/`clippy -p shamir-engine --lib`; query/validator clippy pinned on the no-default and test-util cells; the `--all-targets` pair only in default-plus. `engine-features-fast` is the draft shadow (cells 1 and 5). At R4 the EXPECTED map gains forwarded expectations for the three new crates, mirroring today's query/validator forwarding (`{"default"}` + test-util on the test-util/fjall-test-util/default-plus cells), verified via a throwaway probe crate with `cargo tree` — "no defaults is proven, not labelled".

### 9.3 Loom

The loom module + feature move to `shamir-engine-table` (`WriterDrainBarrier` + `table/support/guards/tests/loom_model.rs`); `crates/engine/shamir-engine/build.rs` emits `cargo::rustc-cfg=loom` when `CARGO_FEATURE_LOOM` is set so dependents don't enter loom paths — this emission moves with the module (or is duplicated per the R4 note). The `loom-model` job (`cargo nextest list/run -p ... --features loom --lib -E 'test(loom_model)'` + `scripts/ci/gates/loom_preflight_check.py`, `--no-tests=fail`) re-points to `shamir-engine-table`. The orderings pin in `.rust-guard/atomic_orderings.toml` for `WriterDrainBarrier` moves WITH the file; pin path update required.

### 9.4 Rustdoc gate

The `rustdoc` job runs SIX `cargo doc` commands today (shamir-engine default & `--no-default-features`; shamir-engine-query default & `--features test-util`; shamir-engine-validator default & `--features test-util`) with `RUSTDOCFLAGS=-D rustdoc::broken_intra_doc_links -D rustdoc::private_intra_doc_links`. Post-split it gains the three new crates (one command each at adoption → nine commands; test-util doc variants are an optional follow-up). The `fast` job's doctest step (`cargo test --doc -p shamir-engine --locked`) and the draft rustdoc gate re-point to `shamir-engine-table` per §7.4.

### 9.5 Benches / examples / tests

`examples/` (`count_allocs_read_pipeline.rs`, `persisted_hnsw.rs`, `prof_order_by.rs`, `vector_report.rs`) and the `[[bench]]` table stay on the facade; `required-features` (`test-util`/`fjall`) keep resolving via forwarded features — verify at R4. Tests move WITH their modules (`discovery_delta`; TEST_LOST is never declarable): `validator/engine_bound/tests/` (fold, R1), `db_instance/migration/tests/` (module move, R3), `repo/seed/install/tests/` (splits with its subjects, R3), `table_store/tests/names.rs` (fn move, R3). `.config/nextest.toml` has NO override targeting shamir-engine today — none needed post-split.

### 9.6 CI matrix, structure gate, layout

`scripts/ci/matrix/test_matrix.py` + `selection.py` map changed paths → nearest owning package (cargo metadata manifest dirs) → role() → production hits trigger reverse-dependency closure. A table-only PR today already selects `shamir-engine` + its reverse closure; post-R4 it selects `shamir-engine-table` + facade + closure — automatically, no `edges.toml` change (§7.4). `docs/*.md` classify as "none". The structure gate (no local `use`, no inline `cfg(test)` mods) applies to the fold and bundle moves — `table/validator_bound/` follows the `tests/` dir layout. `scripts/ci/check_layout.py`: `docs` is in `EXCLUDED_DIR_NAMES` (any depth) and `SOURCE_EXTS` covers `.rs/.ts/.py/.sh`, so this document is out of scope for line limits; layout limits (≤7 entries/folder, ≤1000 lines/file) DO apply to all new/changed source — `table/catalog/`, `table/validator_bound/`, `tx/core.rs` must respect them; the facade shrinks well under the limits. Pre-push hook (`fmt --all --check`, `clippy --workspace --all-targets -D warnings`, `check_layout.py`, cargo sweep) is unaffected structurally.

### 9.7 Landing order

R1 ∥ R3 (migration chunk after R1) → R2 (R2a→R2b→R2c→R2d) → R4 (§7.5).

### 9.8 What the orchestrator's CI must observe (nothing was compiled locally)

1. `engine-features` green per cell with the forwarded EXPECTED entries; `engine-features-fast` shadow cells.
2. `rustdoc` green across the expanded command set (`-D` intra-doc-link lints).
3. `loom-model` green against `shamir-engine-table` (preflight + run).
4. The doctest step green against its re-pointed crate.
5. `test-discovery` `test-util-fts` flags resolve via forwarded features.
6. `merge-gate` (rustdoc + engine-features + loom-model) green at each wave.
7. Reverse-dependency closure from the matrix picks up the new crates without `edges.toml` edits.
8. `stress-nightly` `./scripts/test.sh @oracle --full --profile stress` unaffected semantically.
9. rust-guard conservation/structure gates green per wave with the declared deltas.
10. `#[cfg(test)]` reference `tx::tests::journal_publish_order_tests` resolves through R2; revisited at R4.

## 10. Non-goals

- No source change in THIS task — this document is the only deliverable.
- No crate creation until R4; R1–R3 are in-crate restructurings.
- No version bumps; no `Cargo.toml` dependency changes before R4.
- No `scripts/ci/matrix/edges.toml` changes.
- Batch is NOT extracted (rejected cut, §4.1); `db_instance` stays in the facade; benches/examples/integration tests stay on the facade.
- No new `dyn` traits on write/read hot paths; no eager-init of any `OnceCell`/`OnceLock` semantics.
- No CI workflow edits in this task (R4 owns them, per §7.4).

## Appendix A — Deviations found while writing

Hard evidence collected (read-only) on this worktree that adjusts the rendered design; nothing below silently changes a decision.

**A.1 — `begin_migration_freeze` HAS a facade-external caller.** Grep over `crates/` outside `crates/engine/shamir-engine`: `crates/engine/shamir-db/src/shamir_db/execute/admin_db/admin_migration.rs:151` calls `.begin_migration_freeze_admitted(&src_permit)` (plus table-internal test callers under `src/table/tests/`). §5.11 therefore adds the facade-side `CommitError`-returning wrapper as REQUIRED, not conditional. The claim "no callers outside crates/engine/shamir-engine" is false as stated; the error-split itself stands.

**A.2 — `MigrationCoordinator` is constructed by table code; the whole migration module moves.** `admitted_ops.rs` calls `MigrationCoordinator::new` (:267, :306) with builder chains and returns `Arc<MigrationCoordinator>`; `read_only_views.rs:175` calls `MigrationCoordinator::load_state`; `MigrationState` is defined in `coordinator.rs:39`; `coordinator.rs`'s only in-crate deps are `crate::repo::{RepoWriteDomain, RepoWritePermit}` and `crate::table::MigrationFreezeGuard`. The design's "MigrationCoordinator STAYS in db_instance" is contradicted by this evidence; §5.15 moves the entire `db_instance/migration/` tree (incl. `tests/`) into `table/migration/` and keeps `shamir_engine::migration` / `shamir_engine::db_instance::migration::*` alive via facade re-exports (external shamir-db paths verified: `admin_migration.rs:6`, `tests/rename_migration/migrations/migration_index.rs:20`).

**A.3 — instance→query-batch cannot reach 0 without breaking dependants.** `transactions.rs`'s four implicit-batch methods are called BY METHOD NAME from dependant production code: 7 sites in `crates/engine/shamir-db/src/shamir_db/` (§9.1 list) plus test/e2e references (shamir-db `procedure_e2e.rs:199,240`; shamir-server `version_cas_e2e.rs` references). Removing the shell methods (the design's "move the affected method up to batch" taken literally) would churn two dependant crates. Pick recorded: bodies move to `query/batch/implicit.rs` as free fns; the shell keeps one-line delegating methods; the acceptance pair instance→query-batch lands at **1 residual file** instead of 0. The residual is downward-legal under the corrected order (the gate never flags it; §8.1 carries no baseline entry for it) and is tracked as a documented permanent cleanliness residual, not a gate entry. Everything else in the pair (fk_reverse_cache, the `WriteResult` leaf shadow, the doc link) reaches 0 as designed.

**A.4 — instance→query-leaf file enumeration.** Grep over `repo/instance` finds the leaf/query-shadow refs in exactly `transactions.rs:3` (`crate::query::write::WriteResult`, true leaf) and `fk_reverse_cache.rs:55` (`crate::query::TableRef`, `shamir_query_types` shadow) — consistent with the acceptance pair "instance→query-leaf 2→0" at reference level (both files fixed: R2a + R2c). No third file found; `repo_instance.rs:39` is a doc-comment-only reference, fixed in the R2c pass.

**A.5 — R3 is not fully file-independent of R1.** The design states "nothing in R3 depends on R1/R2 file-wise". `read_only_views.rs` is in R1's file set (leaf-shadow + validator edges) AND is touched by R3's migration-module move (its `crate::migration::MigrationCoordinator::load_state` reference, :175). Constraint adopted: R3's migration chunk lands after R1 merges (§7.3, §7.5).

**A.6 — verified non-deviations (recorded for completeness).** (1) `shamir-engine-query/Cargo.toml` has no `shamir-engine` dependency (deps: shamir-types, shamir-collections, shamir-storage, shamir-query-types, shamir-funclib, shamir-tx, shamir-index) — table→engine-query is acyclic. (2) `shamir-engine-validator/Cargo.toml` likewise has no engine dep. (3) `engine_bound` HAS a `tests/` dir — moves with the fold (§5.5). (4) Seed files call only shell methods (`flush_seed_metadata`) — no R2b dependency for R3 (§5.14). (5) `repo/mod.rs:13` and `:35`, `lib.rs:27/29/39-41`, `query/mod.rs:435-453` match the quoted re-export claims. (6) Design-doc naming in `docs/dev-artifacts/design/` is kebab-case (`*-decision.md`, `*-adr.md`, `*-plan.md` suffixes common); this file follows the mandated name `engine-layering.md` with a status/date/scope header.
