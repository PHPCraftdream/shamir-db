# Part 4 — repository and crate-root sweep

This part owns all tracked engine files except `src/query/**`, `src/validator/**`, `benches/**`, `tests/**`, `src/table/**`, `src/tx/**`, and `src/meta/**`. Generated `part4.json` contains all 114 scope mappings, 4 owned baseline rows, stages, logical paths, actions and exact RepoInstance spans; `render_part4.py` writes those machine-generated rows here.

**OBSERVED:** engine baseline has 51 violations; this part owns 4 rows: `src` direct entries=12 (F), `src/repo` direct entries=19 (D1), `src/repo/tests` direct entries=21 (D1), and `src/repo/repo_instance.rs`=1,967 lines (D1). The three Q-owned baseline entries for benches, root benches and integration tests are not duplicated here. Other exclusions are assigned to the relevant stage owner. Inventory `oversized_sources` has a non-source README, so it is not the authoritative source-only count.

**OBSERVED:** `src/repo/repo_instance.rs` has 1,967 lines; its inherent impl starts at 236. Actual `#[path]` attributes occur at lines 1,37,40,43,46. Restricted items: `pub(super) repo` line 58; `pub(super) write_domain` line 177; `pub(super) read_changelog_bounded_impl` line 1521. Additional actual repo module mounts in `repo/changelog_store.rs` at 16,19,22. The inventory's regex catches comments too; use the source attrs above. The existing `.rust-guard/owned_impl.toml:8–10` already selects `crate::repo::instance::repo_instance` as owner; no guard modifications.

Exact inclusive split partition, every line covered once: `repo_instance.rs` ranges 1–234 (definition/Clone), 235–450 (constructor/startup methods), 451–493 (accessors), 494–610 (replication read/table catalog), 612–714 (complete table-rename methods), 715–1062 (identity/interner/gate/WAL), 1063–1317 (tx API), 1318–1600 (journal), 1601–1967 (indexes/recovery/GC). Sizes are 234,216,43,117,103,348,255,283,367 (max367). Ranges align complete method/item boundaries and keep definition at `repo/instance/repo_instance.rs`; its child files are descendants under `{catalog,transactions,journal,lifecycle}`. All outputs <=1,000 with header budget; no private fields widened.

**OBSERVED 13 RepoInstance implementation locations:** definition source, `visibility_observer.rs:12,156`, `seed_metadata.rs:6`, `seed_install/backing.rs:10`, `changelog_store/{follower_durability.rs:9,write_domain_membership.rs:9,repo_membership_impl.rs:21}`, `clean_horizon/{repo_impl.rs:12,shutdown_impl.rs:60}`, `cold_reconcile/repo_impl.rs:6`, `durability_policy/durability_policy/repo_impl.rs:10`, and Tx worker `tx/follower_durability/receipt/resume/worker.rs:362` (reserved destination `repo/instance/repo_instance/transactions/receipt_worker.rs`, owned/extracted by Tx worker, not duplicated here). The 12 repo-source sites are tabled below; the external Tx source is an observed reservation. Confirm AST at execution. `visibility_observer.rs` contains free algorithms between two impl blocks; `seed_metadata.rs` contains unrelated seed data/types. Extract only impl blocks, never mixed whole files. Audit all restricted visibility in moved items for changed `super` meaning.

## Root packing / index and migration

**DECIDED final src direct entries = 7:** `lib.rs`, `db_instance`, `query`, `repo`, `table`, `tx`, `validator`. Move meta out of src at the part-3 D3 location (proposed `repo/meta`); migration beneath `db_instance/migration`; move `src/LAYERS.md` and `src/README.md` to crate-root docs. Cargo.toml, crate README, build.rs and examples stay at crate root. Remove physical index shim atomically with facade conversion.

Index and index2 have different shapes (`lib.rs:13–14`; `src/index/mod.rs:1–11`). Preserve the index interface as an inline `pub mod index { ... }` with shape-specific re-exports, independently retain `pub use shamir_index as index2`; do not use a bare alias. The deleted `index/mod.rs` row has no physical destination (empty destination/action), because code is inserted in existing lib.rs. Move index docs to unique `docs/index_*` destinations. Verify public names from source before implementation. Nine migration source files move individually; logical paths become `crate::db_instance::migration::*`. No arbitrary baseline removals.

Capacity certification requires union with other parts' output tree, including part3 meta and exactly one Tx receipt-worker output. This part alone does not claim every descendant directory <=7.

## Guard campaigns (proposal only; no .rust-guard edits)

Observed: `.rust-guard/stages.toml` requires exact `name`, `packages`, `delta`, currently campaigns empty. `.rust-guard/owned_impl.toml` already holds intended owner. `tools/rust-guard/README.md:243–302`: delta requires `base` SHA and entries require nonempty `path`, `item`, `reason`, `owner`; `[[moved]]` accepts `to_path`/`to_item`; `[[extracted]]` accepts `to_item`; test loss is never permitted.

Exact proposed tuples stage / name / package list / delta path (matching the committed engine-sweep convention):
A1 / `engine-sweep-a1` / `["shamir-engine","shamir-db","shamir-server"]` / `.rust-guard/stages/engine-sweep-a1.toml`;
A2 / `engine-sweep-a2` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-a2.toml`;
B1 / `engine-sweep-b1` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-b1.toml`;
B2 / `engine-sweep-b2` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-b2.toml`;
C1 / `engine-sweep-c1` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-c1.toml`;
C2 / `engine-sweep-c2` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-c2.toml`;
D1 / `engine-sweep-d1` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-d1.toml`;
D2 / `engine-sweep-d2` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-d2.toml`;
D3 / `engine-sweep-d3` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-d3.toml`;
E1 / `engine-sweep-e1` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-e1.toml`;
E2 / `engine-sweep-e2` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-e2.toml`;
F / `engine-sweep-f` / `["shamir-engine","shamir-db","shamir-server"]` / `.rust-guard/stages/engine-sweep-f.toml`;
Q / `engine-sweep-q` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-q.toml`;
T / `engine-sweep-t` / `["shamir-engine"]` / `.rust-guard/stages/engine-sweep-t.toml`.

Parser-valid delta example (actual SHA and exact identities required before activation):

```toml
base = "<merge-base SHA>"
[[moved]]
path = "crates/shamir-engine/src/repo/visibility_observer.rs"
item = "shamir_engine::repo::RepoInstance::observe_visibility"
to_path = "crates/shamir-engine/src/repo/instance/repo_instance/catalog/visibility.rs"
to_item = "shamir_engine::repo::instance::repo_instance::RepoInstance::observe_visibility"
reason = "Extract impl below defining module while retaining free algorithms"
owner = "@engine-sweep"
```

Downstream hook consumer observed in the adjudicated plan: `crates/shamir-db/src/shamir_db/tests/shutdown_quiesce_tests.rs:9`; confirm before A1. Bench/test files are separate Cargo targets, not automatically module descendants; preserve required features and discovery. No Rust/config/baseline/workflow/guard mutation; no cargo/build/test commands.

## Complete mapping rows generated from tracked files

| Source -> destination | Stage | projected lines | logical old -> new | action |
|---|---|---:|---|---|
## Complete source mapping rows (generated)

| Source | Destination/action | Stage | Lines | Logical old | Logical new |
|---|---|---|---:|---|---|
| `crates/shamir-engine/Cargo.toml` | `crates/shamir-engine/Cargo.toml`; retained | F | 321 | `non-Rust tracked path` | `non-Rust tracked path` |
| `crates/shamir-engine/build.rs` | `crates/shamir-engine/build.rs`; retained | F | 16 | `target-root build` | `target-root build` |
| `crates/shamir-engine/examples/count_allocs_read_pipeline.rs` | `crates/shamir-engine/examples/count_allocs_read_pipeline.rs`; retained | F | 280 | `target-root examples::count_allocs_read_pipeline` | `target-root examples::count_allocs_read_pipeline` |
| `crates/shamir-engine/examples/persisted_hnsw.rs` | `crates/shamir-engine/examples/persisted_hnsw.rs`; retained | F | 422 | `target-root examples::persisted_hnsw` | `target-root examples::persisted_hnsw` |
| `crates/shamir-engine/examples/prof_order_by.rs` | `crates/shamir-engine/examples/prof_order_by.rs`; retained | F | 334 | `target-root examples::prof_order_by` | `target-root examples::prof_order_by` |
| `crates/shamir-engine/examples/vector_report.rs` | `crates/shamir-engine/examples/vector_report.rs`; retained | F | 480 | `target-root examples::vector_report` | `target-root examples::vector_report` |
| `crates/shamir-engine/src/LAYERS.md` | `crates/shamir-engine/LAYERS.md`; relocate documentation | F | 105 | `non-Rust tracked path` | `non-module documentation` |
| `crates/shamir-engine/src/README.md` | `crates/shamir-engine/README.md`; relocate documentation | F | 130 | `non-Rust tracked path` | `non-module documentation` |
| `crates/shamir-engine/src/db_instance/README.md` | `crates/shamir-engine/src/db_instance/README.md`; retained | F | 83 | `non-Rust tracked path` | `non-Rust tracked path` |
| `crates/shamir-engine/src/db_instance/db_instance.rs` | `crates/shamir-engine/src/db_instance/db_instance.rs`; retained | F | 381 | `crate::db_instance::db_instance` | `crate::db_instance::db_instance` |
| `crates/shamir-engine/src/db_instance/mod.rs` | `crates/shamir-engine/src/db_instance/mod.rs`; retained | F | 5 | `crate::db_instance` | `crate::db_instance` |
| `crates/shamir-engine/src/db_instance/tests/db_instance_guard_across_await_tests.rs` | `crates/shamir-engine/src/db_instance/tests/db_instance_guard_across_await_tests.rs`; retained | F | 134 | `crate::db_instance::tests::db_instance_guard_across_await_tests` | `crate::db_instance::tests::db_instance_guard_across_await_tests` |
| `crates/shamir-engine/src/db_instance/tests/db_instance_tests.rs` | `crates/shamir-engine/src/db_instance/tests/db_instance_tests.rs`; retained | F | 301 | `crate::db_instance::tests::db_instance_tests` | `crate::db_instance::tests::db_instance_tests` |
| `crates/shamir-engine/src/db_instance/tests/mod.rs` | `crates/shamir-engine/src/db_instance/tests/mod.rs`; retained | F | 3 | `crate::db_instance::tests` | `crate::db_instance::tests` |
| `crates/shamir-engine/src/db_instance/tests/write_domain_membership_tests.rs` | `crates/shamir-engine/src/db_instance/tests/write_domain_membership_tests.rs`; retained | F | 190 | `crate::db_instance::tests::write_domain_membership_tests` | `crate::db_instance::tests::write_domain_membership_tests` |
| `crates/shamir-engine/src/index/README.md` | `crates/shamir-engine/docs/index_README.md`; relocate documentation | F | 116 | `non-Rust tracked path` | `non-module documentation` |
| `crates/shamir-engine/src/index/index_types.md` | `crates/shamir-engine/docs/index_index_types.md`; relocate documentation | F | 381 | `non-Rust tracked path` | `non-module documentation` |
| `crates/shamir-engine/src/index/mod.rs` | `(deleted; inline facade)`; delete shim; replace atomically with inline compatibility module | F | 11 | `crate::index` | `crate::index inline facade` |
| `crates/shamir-engine/src/lib.rs` | `crates/shamir-engine/src/lib.rs`; retained | F | 27 | `crate::lib` | `crate::lib` |
| `crates/shamir-engine/src/migration/coordinator.rs` | `crates/shamir-engine/src/db_instance/migration/coordinator.rs`; move below db_instance | F | 683 | `crate::migration::coordinator` | `crate::db_instance::migration::coordinator` |
| `crates/shamir-engine/src/migration/mod.rs` | `crates/shamir-engine/src/db_instance/migration/mod.rs`; move below db_instance | F | 46 | `crate::migration` | `crate::db_instance::migration::mod` |
| `crates/shamir-engine/src/migration/shadow_key.rs` | `crates/shamir-engine/src/db_instance/migration/shadow_key.rs`; move below db_instance | F | 78 | `crate::migration::shadow_key` | `crate::db_instance::migration::shadow_key` |
| `crates/shamir-engine/src/migration/shadow_log.rs` | `crates/shamir-engine/src/db_instance/migration/shadow_log.rs`; move below db_instance | F | 232 | `crate::migration::shadow_log` | `crate::db_instance::migration::shadow_log` |
| `crates/shamir-engine/src/migration/tests/coordinator_tests.rs` | `crates/shamir-engine/src/db_instance/migration/tests/coordinator_tests.rs`; move below db_instance | F | 669 | `crate::migration::tests::coordinator_tests` | `crate::db_instance::migration::coordinator_tests` |
| `crates/shamir-engine/src/migration/tests/cutover_guard_tests.rs` | `crates/shamir-engine/src/db_instance/migration/tests/cutover_guard_tests.rs`; move below db_instance | F | 184 | `crate::migration::tests::cutover_guard_tests` | `crate::db_instance::migration::cutover_guard_tests` |
| `crates/shamir-engine/src/migration/tests/mod.rs` | `crates/shamir-engine/src/db_instance/migration/tests/mod.rs`; move below db_instance | F | 4 | `crate::migration::tests` | `crate::db_instance::migration::mod` |
| `crates/shamir-engine/src/migration/tests/shadow_key_tests.rs` | `crates/shamir-engine/src/db_instance/migration/tests/shadow_key_tests.rs`; move below db_instance | F | 57 | `crate::migration::tests::shadow_key_tests` | `crate::db_instance::migration::shadow_key_tests` |
| `crates/shamir-engine/src/migration/tests/shadow_log_tests.rs` | `crates/shamir-engine/src/db_instance/migration/tests/shadow_log_tests.rs`; move below db_instance | F | 349 | `crate::migration::tests::shadow_log_tests` | `crate::db_instance::migration::shadow_log_tests` |
| `crates/shamir-engine/src/repo/README.md` | `crates/shamir-engine/docs/repo_README.md`; relocate documentation | D1 | 122 | `non-Rust tracked path` | `non-module documentation` |
| `crates/shamir-engine/src/repo/bounded_journal_read.rs` | `crates/shamir-engine/src/repo/journal/bounded_journal_read.rs`; regroup repository modules | D1 | 55 | `crate::repo::bounded_journal_read` | `crate::repo::journal::bounded_journal_read` |
| `crates/shamir-engine/src/repo/changelog_store.rs` | `crates/shamir-engine/src/repo/journal/mod.rs`; regroup repository modules | D2 | 147 | `crate::repo::changelog_store` | `crate::repo::journal` |
| `crates/shamir-engine/src/repo/changelog_store/follower_durability.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/transactions/follower_durability.rs`; extract inherent impl into owner descendant | D2 | 41 | `crate::repo::changelog_store::follower_durability` | `crate::repo::instance::repo_instance::transactions::follower_durability` |
| `crates/shamir-engine/src/repo/changelog_store/repo_membership_impl.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs`; extract inherent impl into owner descendant | D2 | 329 | `crate::repo::changelog_store::repo_membership_impl` | `crate::repo::instance::repo_instance::catalog::repo_membership_impl` |
| `crates/shamir-engine/src/repo/changelog_store/tests/mod.rs` | `crates/shamir-engine/src/repo/journal/changelog_store/tests/mod.rs`; regroup repository modules | D2 | 1 | `crate::repo::changelog_store::tests` | `crate::repo::journal::changelog_store::tests` |
| `crates/shamir-engine/src/repo/changelog_store/tests/write_domain_tests.rs` | `crates/shamir-engine/src/repo/journal/changelog_store/tests/write_domain_tests.rs`; regroup repository modules | D2 | 103 | `crate::repo::changelog_store::tests::write_domain_tests` | `crate::repo::journal::changelog_store::tests::write_domain_tests` |
| `crates/shamir-engine/src/repo/changelog_store/write_domain.rs` | `crates/shamir-engine/src/repo/journal/changelog_store/write_domain.rs`; regroup repository modules | D2 | 277 | `crate::repo::changelog_store::write_domain` | `crate::repo::journal::changelog_store::write_domain` |
| `crates/shamir-engine/src/repo/changelog_store/write_domain_membership.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/transactions/write_domain_membership.rs`; extract inherent impl into owner descendant | D2 | 34 | `crate::repo::changelog_store::write_domain_membership` | `crate::repo::instance::repo_instance::transactions::write_domain_membership` |
| `crates/shamir-engine/src/repo/clean_horizon/repo_impl.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/clean_horizon_repo_impl.rs`; move inherent impl into owner descendant | D1 | 70 | `crate::repo::clean_horizon::repo_impl` | `crate::repo::instance::repo_instance::lifecycle::clean_horizon_repo_impl` |
| `crates/shamir-engine/src/repo/clean_horizon/shutdown_impl.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/recovery/clean_horizon_shutdown_impl.rs`; move inherent impl into owner descendant | D1 | 313 | `crate::repo::clean_horizon::shutdown_impl` | `crate::repo::instance::repo_instance::lifecycle::recovery::clean_horizon_shutdown_impl` |
| `crates/shamir-engine/src/repo/cold_reconcile/repo_impl.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/recovery/cold_reconcile_repo_impl.rs`; move inherent impl into owner descendant | D1 | 31 | `crate::repo::cold_reconcile::repo_impl` | `crate::repo::instance::repo_instance::lifecycle::recovery::cold_reconcile_repo_impl` |
| `crates/shamir-engine/src/repo/durability_policy/durability_metrics.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_metrics.rs`; regroup repository modules | D2 | 177 | `crate::repo::durability_policy::durability_metrics` | `crate::repo::journal::backend::durability_policy::durability_metrics` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy.rs`; regroup repository modules | D2 | 115 | `crate::repo::durability_policy::durability_policy` | `crate::repo::journal::backend::durability_policy::durability_policy` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/commit_policy.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/commit_policy.rs`; regroup repository modules | D2 | 46 | `crate::repo::durability_policy::durability_policy::commit_policy` | `crate::repo::journal::backend::durability_policy::durability_policy::commit_policy` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/recoverability_coverage.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/recoverability_coverage.rs`; regroup repository modules | D2 | 118 | `crate::repo::durability_policy::durability_policy::recoverability_coverage` | `crate::repo::journal::backend::durability_policy::durability_policy::recoverability_coverage` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/recovered_open.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/recovered_open.rs`; regroup repository modules | D2 | 45 | `crate::repo::durability_policy::durability_policy::recovered_open` | `crate::repo::journal::backend::durability_policy::durability_policy::recovered_open` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/replication_page.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/replication_page.rs`; regroup repository modules | D2 | 64 | `crate::repo::durability_policy::durability_policy::replication_page` | `crate::repo::journal::backend::durability_policy::durability_policy::replication_page` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/repo_impl.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/durability_repo_impl.rs`; move inherent impl into owner descendant | D2 | 296 | `crate::repo::durability_policy::durability_policy::repo_impl` | `crate::repo::instance::repo_instance::lifecycle::durability_repo_impl` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/segment_bytes.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/segment_bytes.rs`; regroup repository modules | D2 | 17 | `crate::repo::durability_policy::durability_policy::segment_bytes` | `crate::repo::journal::backend::durability_policy::durability_policy::segment_bytes` |
| `crates/shamir-engine/src/repo/durability_policy/mod.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/mod.rs`; regroup repository modules | D2 | 52 | `crate::repo::durability_policy` | `crate::repo::journal::backend::durability_policy` |
| `crates/shamir-engine/src/repo/durability_policy/replication_bound.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/replication_bound.rs`; regroup repository modules | D2 | 109 | `crate::repo::durability_policy::replication_bound` | `crate::repo::journal::backend::durability_policy::replication_bound` |
| `crates/shamir-engine/src/repo/durability_policy/resolve.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/resolve.rs`; regroup repository modules | D2 | 89 | `crate::repo::durability_policy::resolve` | `crate::repo::journal::backend::durability_policy::resolve` |
| `crates/shamir-engine/src/repo/durability_policy/tests/batch_synced_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/batch_synced_tests.rs`; regroup repository modules | D2 | 60 | `crate::repo::durability_policy::tests::batch_synced_tests` | `crate::repo::journal::backend::durability_policy::tests::batch_synced_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/commit_policy_tests/commit_policy_independent_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/commit_policy_tests/commit_policy_independent_tests.rs`; regroup repository modules | D2 | 175 | `crate::repo::durability_policy::tests::commit_policy_tests::commit_policy_independent_tests` | `crate::repo::journal::backend::durability_policy::tests::commit_policy_tests::commit_policy_independent_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/commit_policy_tests/commit_policy_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/commit_policy_tests/commit_policy_tests.rs`; regroup repository modules | D2 | 181 | `crate::repo::durability_policy::tests::commit_policy_tests::commit_policy_tests` | `crate::repo::journal::backend::durability_policy::tests::commit_policy_tests::commit_policy_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/commit_policy_tests/mod.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/commit_policy_tests/mod.rs`; regroup repository modules | D2 | 6 | `crate::repo::durability_policy::tests::commit_policy_tests` | `crate::repo::journal::backend::durability_policy::tests::commit_policy_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/commit_policy_tests/repo_override_commit_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/commit_policy_tests/repo_override_commit_tests.rs`; regroup repository modules | D2 | 80 | `crate::repo::durability_policy::tests::commit_policy_tests::repo_override_commit_tests` | `crate::repo::journal::backend::durability_policy::tests::commit_policy_tests::repo_override_commit_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/helpers.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/helpers.rs`; regroup repository modules | D2 | 143 | `crate::repo::durability_policy::tests::helpers` | `crate::repo::journal::backend::durability_policy::tests::helpers` |
| `crates/shamir-engine/src/repo/durability_policy/tests/mem/mem_sink_hybrid_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/mem/mem_sink_hybrid_tests.rs`; regroup repository modules | D2 | 80 | `crate::repo::durability_policy::tests::mem::mem_sink_hybrid_tests` | `crate::repo::journal::backend::durability_policy::tests::mem::mem_sink_hybrid_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/mem/mem_sink_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/mem/mem_sink_tests.rs`; regroup repository modules | D2 | 106 | `crate::repo::durability_policy::tests::mem::mem_sink_tests` | `crate::repo::journal::backend::durability_policy::tests::mem::mem_sink_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/mem/mod.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/mem/mod.rs`; regroup repository modules | D2 | 3 | `crate::repo::durability_policy::tests::mem` | `crate::repo::journal::backend::durability_policy::tests::mem` |
| `crates/shamir-engine/src/repo/durability_policy/tests/mod.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/mod.rs`; regroup repository modules | D2 | 9 | `crate::repo::durability_policy::tests` | `crate::repo::journal::backend::durability_policy::tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/power_loss_bound_tests/bookmark_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/power_loss_bound_tests/bookmark_tests.rs`; regroup repository modules | D2 | 115 | `crate::repo::durability_policy::tests::power_loss_bound_tests::bookmark_tests` | `crate::repo::journal::backend::durability_policy::tests::power_loss_bound_tests::bookmark_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/power_loss_bound_tests/mod.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/power_loss_bound_tests/mod.rs`; regroup repository modules | D2 | 2 | `crate::repo::durability_policy::tests::power_loss_bound_tests` | `crate::repo::journal::backend::durability_policy::tests::power_loss_bound_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/power_loss_bound_tests/reopen_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/power_loss_bound_tests/reopen_tests.rs`; regroup repository modules | D2 | 502 | `crate::repo::durability_policy::tests::power_loss_bound_tests::reopen_tests` | `crate::repo::journal::backend::durability_policy::tests::power_loss_bound_tests::reopen_tests` |
| `crates/shamir-engine/src/repo/durability_policy/tests/replication_s_bound_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/durability_policy/tests/replication_s_bound_tests.rs`; regroup repository modules | D2 | 359 | `crate::repo::durability_policy::tests::replication_s_bound_tests` | `crate::repo::journal::backend::durability_policy::tests::replication_s_bound_tests` |
| `crates/shamir-engine/src/repo/fk_reverse_cache.rs` | `crates/shamir-engine/src/repo/instance/support/catalog/fk_reverse_cache.rs`; regroup repository modules | D1 | 515 | `crate::repo::fk_reverse_cache` | `crate::repo::instance::support::catalog::fk_reverse_cache` |
| `crates/shamir-engine/src/repo/group_commit/group_commit.rs` | `crates/shamir-engine/src/repo/journal/backend/group_commit/group_commit.rs`; regroup repository modules | D2 | 174 | `crate::repo::group_commit::group_commit` | `crate::repo::journal::backend::group_commit::group_commit` |
| `crates/shamir-engine/src/repo/group_commit/mod.rs` | `crates/shamir-engine/src/repo/journal/backend/group_commit/mod.rs`; regroup repository modules | D2 | 6 | `crate::repo::group_commit` | `crate::repo::journal::backend::group_commit` |
| `crates/shamir-engine/src/repo/group_commit/tests/group_commit_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/group_commit/tests/group_commit_tests.rs`; regroup repository modules | D2 | 98 | `crate::repo::group_commit::tests::group_commit_tests` | `crate::repo::journal::backend::group_commit::tests::group_commit_tests` |
| `crates/shamir-engine/src/repo/group_commit/tests/leader_cancel_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/group_commit/tests/leader_cancel_tests.rs`; regroup repository modules | D2 | 110 | `crate::repo::group_commit::tests::leader_cancel_tests` | `crate::repo::journal::backend::group_commit::tests::leader_cancel_tests` |
| `crates/shamir-engine/src/repo/group_commit/tests/mod.rs` | `crates/shamir-engine/src/repo/journal/backend/group_commit/tests/mod.rs`; regroup repository modules | D2 | 3 | `crate::repo::group_commit::tests` | `crate::repo::journal::backend::group_commit::tests` |
| `crates/shamir-engine/src/repo/group_commit/tests/panicking_flush_tests.rs` | `crates/shamir-engine/src/repo/journal/backend/group_commit/tests/panicking_flush_tests.rs`; regroup repository modules | D2 | 109 | `crate::repo::group_commit::tests::panicking_flush_tests` | `crate::repo::journal::backend::group_commit::tests::panicking_flush_tests` |
| `crates/shamir-engine/src/repo/mod.rs` | `crates/shamir-engine/src/repo/mod.rs`; retained | D1 | 37 | `crate::repo` | `crate::repo` |
| `crates/shamir-engine/src/repo/prepare_seed.rs` | `crates/shamir-engine/src/repo/seed/prepare_seed.rs`; regroup repository modules | D3 | 75 | `crate::repo::prepare_seed` | `crate::repo::seed::prepare_seed` |
| `crates/shamir-engine/src/repo/repo_config.rs` | `crates/shamir-engine/src/repo/instance/config.rs`; regroup repository modules | D1 | 29 | `crate::repo::repo_config` | `crate::repo::instance::config` |
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance.rs`; split into defining module plus descendant topic modules | D1 | 234 | `crate::repo::repo_instance` | `crate::repo::instance::repo_instance` |
| `crates/shamir-engine/src/repo/repo_types.rs` | `crates/shamir-engine/src/repo/journal/backend/repo_types.rs`; regroup repository modules | D1 | 623 | `crate::repo::repo_types` | `crate::repo::journal::backend::repo_types` |
| `crates/shamir-engine/src/repo/seed_install/backing.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/seed_backing_impl.rs`; extract inherent impl into owner descendant | D3 | 25 | `crate::repo::seed_install::backing` | `crate::repo::instance::repo_instance::lifecycle::seed_backing_impl` |
| `crates/shamir-engine/src/repo/seed_install/inventory.rs` | `crates/shamir-engine/src/repo/seed/install/inventory.rs`; regroup repository modules | D3 | 56 | `crate::repo::seed_install::inventory` | `crate::repo::seed::install::inventory` |
| `crates/shamir-engine/src/repo/seed_install/mod.rs` | `crates/shamir-engine/src/repo/seed/install/mod.rs`; regroup repository modules | D3 | 12 | `crate::repo::seed_install` | `crate::repo::seed::install` |
| `crates/shamir-engine/src/repo/seed_install/receipt.rs` | `crates/shamir-engine/src/repo/seed/install/receipt.rs`; regroup repository modules | D3 | 139 | `crate::repo::seed_install::receipt` | `crate::repo::seed::install::receipt` |
| `crates/shamir-engine/src/repo/seed_install/row.rs` | `crates/shamir-engine/src/repo/seed/install/row.rs`; regroup repository modules | D3 | 38 | `crate::repo::seed_install::row` | `crate::repo::seed::install::row` |
| `crates/shamir-engine/src/repo/seed_install/tests/disk_tests.rs` | `crates/shamir-engine/src/repo/seed/install/tests/disk_tests.rs`; regroup repository modules | D3 | 77 | `crate::repo::seed_install::tests::disk_tests` | `crate::repo::seed::install::tests::disk_tests` |
| `crates/shamir-engine/src/repo/seed_install/tests/durability_tests.rs` | `crates/shamir-engine/src/repo/seed/install/tests/durability_tests.rs`; regroup repository modules | D3 | 75 | `crate::repo::seed_install::tests::durability_tests` | `crate::repo::seed::install::tests::durability_tests` |
| `crates/shamir-engine/src/repo/seed_install/tests/flush_probe.rs` | `crates/shamir-engine/src/repo/seed/install/tests/flush_probe.rs`; regroup repository modules | D3 | 96 | `crate::repo::seed_install::tests::flush_probe` | `crate::repo::seed::install::tests::flush_probe` |
| `crates/shamir-engine/src/repo/seed_install/tests/helpers.rs` | `crates/shamir-engine/src/repo/seed/install/tests/helpers.rs`; regroup repository modules | D3 | 70 | `crate::repo::seed_install::tests::helpers` | `crate::repo::seed::install::tests::helpers` |
| `crates/shamir-engine/src/repo/seed_install/tests/image_tests.rs` | `crates/shamir-engine/src/repo/seed/install/tests/image_tests.rs`; regroup repository modules | D3 | 186 | `crate::repo::seed_install::tests::image_tests` | `crate::repo::seed::install::tests::image_tests` |
| `crates/shamir-engine/src/repo/seed_install/tests/mod.rs` | `crates/shamir-engine/src/repo/seed/install/tests/mod.rs`; regroup repository modules | D3 | 8 | `crate::repo::seed_install::tests` | `crate::repo::seed::install::tests` |
| `crates/shamir-engine/src/repo/seed_install/tests/validation_tests.rs` | `crates/shamir-engine/src/repo/seed/install/tests/validation_tests.rs`; regroup repository modules | D3 | 126 | `crate::repo::seed_install::tests::validation_tests` | `crate::repo::seed::install::tests::validation_tests` |
| `crates/shamir-engine/src/repo/seed_install/writer.rs` | `crates/shamir-engine/src/repo/seed/install/writer.rs`; regroup repository modules | D3 | 346 | `crate::repo::seed_install::writer` | `crate::repo::seed::install::writer` |
| `crates/shamir-engine/src/repo/seed_metadata.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/seed_metadata_impl.rs`; extract inherent impl only; retain data/helpers | D3 | 15 | `crate::repo::seed_metadata` | `crate::repo::instance::repo_instance::lifecycle::seed_metadata_impl` |
| `crates/shamir-engine/src/repo/tests/bounded_journal_read_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/journal/bounded_journal_read_tests.rs`; regroup repo tests under journal test modules | D1 | 272 | `crate::repo::tests::bounded_journal_read_tests` | `crate::repo::journal::tests::journal::bounded_journal_read_tests` |
| `crates/shamir-engine/src/repo/tests/box_repo_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/core/box_repo_tests.rs`; regroup repo tests under journal test modules | D1 | 64 | `crate::repo::tests::box_repo_tests` | `crate::repo::journal::tests::core::box_repo_tests` |
| `crates/shamir-engine/src/repo/tests/changelog_gap_marker_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/journal/changelog_gap_marker_tests.rs`; regroup repo tests under journal test modules | D1 | 57 | `crate::repo::tests::changelog_gap_marker_tests` | `crate::repo::journal::tests::journal::changelog_gap_marker_tests` |
| `crates/shamir-engine/src/repo/tests/changelog_store_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/journal/changelog_store_tests.rs`; regroup repo tests under journal test modules | D2 | 143 | `crate::repo::tests::changelog_store_tests` | `crate::repo::journal::tests::journal::changelog_store_tests` |
| `crates/shamir-engine/src/repo/tests/clean_horizon_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/journal/clean_horizon_tests.rs`; regroup repo tests under journal test modules | D1 | 183 | `crate::repo::tests::clean_horizon_tests` | `crate::repo::journal::tests::journal::clean_horizon_tests` |
| `crates/shamir-engine/src/repo/tests/current_version_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/lifecycle/current_version_tests.rs`; regroup repo tests under journal test modules | D1 | 32 | `crate::repo::tests::current_version_tests` | `crate::repo::journal::tests::lifecycle::current_version_tests` |
| `crates/shamir-engine/src/repo/tests/hybrid_repo_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/core/hybrid_repo_tests.rs`; regroup repo tests under journal test modules | D1 | 369 | `crate::repo::tests::hybrid_repo_tests` | `crate::repo::journal::tests::core::hybrid_repo_tests` |
| `crates/shamir-engine/src/repo/tests/hybrid_table_open_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/core/hybrid_table_open_tests.rs`; regroup repo tests under journal test modules | D1 | 472 | `crate::repo::tests::hybrid_table_open_tests` | `crate::repo::journal::tests::core::hybrid_table_open_tests` |
| `crates/shamir-engine/src/repo/tests/mod.rs` | `crates/shamir-engine/src/repo/journal/tests/mod.rs`; regroup repo tests under journal test modules | D1 | 23 | `crate::repo::tests` | `crate::repo::journal::tests::mod` |
| `crates/shamir-engine/src/repo/tests/per_table_mvcc_attach_collision_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/lifecycle/per_table_mvcc_attach_collision_tests.rs`; regroup repo tests under journal test modules | D1 | 105 | `crate::repo::tests::per_table_mvcc_attach_collision_tests` | `crate::repo::journal::tests::lifecycle::per_table_mvcc_attach_collision_tests` |
| `crates/shamir-engine/src/repo/tests/per_table_mvcc_token_names_deadlock_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/lifecycle/per_table_mvcc_token_names_deadlock_tests.rs`; regroup repo tests under journal test modules | D1 | 341 | `crate::repo::tests::per_table_mvcc_token_names_deadlock_tests` | `crate::repo::journal::tests::lifecycle::per_table_mvcc_token_names_deadlock_tests` |
| `crates/shamir-engine/src/repo/tests/prealloc_bounds_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/journal/prealloc_bounds_tests.rs`; regroup repo tests under journal test modules | D1 | 54 | `crate::repo::tests::prealloc_bounds_tests` | `crate::repo::journal::tests::journal::prealloc_bounds_tests` |
| `crates/shamir-engine/src/repo/tests/prepare_seed_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/lifecycle/prepare_seed_tests.rs`; regroup repo tests under journal test modules | D3 | 50 | `crate::repo::tests::prepare_seed_tests` | `crate::repo::journal::tests::lifecycle::prepare_seed_tests` |
| `crates/shamir-engine/src/repo/tests/recovery_gap_marker_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/journal/recovery_gap_marker_tests.rs`; regroup repo tests under journal test modules | D1 | 401 | `crate::repo::tests::recovery_gap_marker_tests` | `crate::repo::journal::tests::journal::recovery_gap_marker_tests` |
| `crates/shamir-engine/src/repo/tests/repl_bookmark_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/identity/repl_bookmark_tests.rs`; regroup repo tests under journal test modules | D1 | 181 | `crate::repo::tests::repl_bookmark_tests` | `crate::repo::journal::tests::identity::repl_bookmark_tests` |
| `crates/shamir-engine/src/repo/tests/repo_config_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/core/repo_config_tests.rs`; regroup repo tests under journal test modules | D1 | 43 | `crate::repo::tests::repo_config_tests` | `crate::repo::journal::tests::core::repo_config_tests` |
| `crates/shamir-engine/src/repo/tests/repo_identity_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/identity/repo_identity_tests.rs`; regroup repo tests under journal test modules | D1 | 272 | `crate::repo::tests::repo_identity_tests` | `crate::repo::journal::tests::identity::repo_identity_tests` |
| `crates/shamir-engine/src/repo/tests/repo_instance_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/core/repo_instance_tests.rs`; regroup repo tests under journal test modules | D1 | 636 | `crate::repo::tests::repo_instance_tests` | `crate::repo::journal::tests::core::repo_instance_tests` |
| `crates/shamir-engine/src/repo/tests/repo_tx_accessors_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/core/repo_tx_accessors_tests.rs`; regroup repo tests under journal test modules | D1 | 213 | `crate::repo::tests::repo_tx_accessors_tests` | `crate::repo::journal::tests::core::repo_tx_accessors_tests` |
| `crates/shamir-engine/src/repo/tests/table_init_retry_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/identity/table_init_retry_tests.rs`; regroup repo tests under journal test modules | D1 | 181 | `crate::repo::tests::table_init_retry_tests` | `crate::repo::journal::tests::identity::table_init_retry_tests` |
| `crates/shamir-engine/src/repo/tests/visibility_observer_tests.rs` | `crates/shamir-engine/src/repo/journal/tests/identity/visibility_observer_tests.rs`; regroup repo tests under journal test modules | D1 | 116 | `crate::repo::tests::visibility_observer_tests` | `crate::repo::journal::tests::identity::visibility_observer_tests` |
| `crates/shamir-engine/src/repo/version_provider.rs` | `crates/shamir-engine/src/repo/instance/version_provider.rs`; regroup repository modules | D1 | 76 | `crate::repo::version_provider` | `crate::repo::instance::version_provider` |
| `crates/shamir-engine/src/repo/visibility_observer.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/catalog/visibility_impl.rs`; extract two impl blocks only; retain free observer algorithms | D1 | 172 | `crate::repo::visibility_observer` | `crate::repo::instance::repo_instance::catalog::visibility_impl` |

## RepoInstance split spans (generated)

| Source | Destination | inclusive lines | output lines | topic | stage |
|---|---|---:|---:|---|---|
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance.rs` | 1-234 | 234 | defining type and Clone impl | D1 |
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/catalog/operations.rs` | 235-493 | 259 | construction, lifecycle and public repository identity/catalog primitives | D1 |
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/catalog/operations.rs` | 494-714 | 221 | replication read and table registration/rename APIs | D1 |
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/state.rs` | 715-1062 | 348 | identity, interner, transaction gate and WAL init | D1 |
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/transactions/api.rs` | 1063-1317 | 255 | transaction entrypoints and replication bookmark | D1 |
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/journal/changefeed.rs` | 1318-1600 | 283 | changefeed, journal reads and progress | D1 |
| `crates/shamir-engine/src/repo/repo_instance.rs` | `crates/shamir-engine/src/repo/instance/repo_instance/catalog/index_gc.rs` | 1601-1967 | 367 | index APIs, recovery, flush and GC | D1 |

## New manifests (generated)

| Destination | lines | stage | kind | declarations |
|---|---:|---|---|---|
| `crates/shamir-engine/src/repo/instance/mod.rs` | 1 | D1 | module_manifest | `mod repo_instance; mod support; pub use repo_instance::RepoInstance;` |
| `crates/shamir-engine/src/repo/instance/support/mod.rs` | 1 | D1 | module_manifest | `mod catalog; mod lifecycle;` |
| `crates/shamir-engine/src/repo/instance/support/catalog/mod.rs` | 1 | D1 | module_manifest | `mod fk_reverse_cache; mod visibility_observer;` |
| `crates/shamir-engine/src/repo/instance/support/lifecycle/mod.rs` | 1 | D1 | module_manifest | `mod clean_horizon; mod cold_reconcile;` |
| `crates/shamir-engine/src/repo/instance/repo_instance/mod.rs` | 1 | D1 | module_manifest | `mod catalog; mod journal; mod lifecycle; mod transactions;` |
| `crates/shamir-engine/src/repo/instance/repo_instance/catalog/mod.rs` | 1 | D1 | module_manifest | `mod index_gc; mod operations; mod repo_membership_impl; mod visibility_impl;` |
| `crates/shamir-engine/src/repo/instance/repo_instance/journal/mod.rs` | 1 | D1 | module_manifest | `mod changefeed;` |
| `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/mod.rs` | 1 | D1 | module_manifest | `mod admission; mod clean_horizon_repo_impl; mod recovery; mod durability_repo_impl; mod seed_backing_impl; mod seed_metadata_impl; mod state;` |
| `crates/shamir-engine/src/repo/instance/repo_instance/transactions/mod.rs` | 1 | D1 | module_manifest | `mod api; mod follower_durability; mod receipt_worker; mod write_domain_membership;` |
| `crates/shamir-engine/src/repo/journal/mod.rs` | 1 | D1 | module_manifest | `mod bounded_journal_read; mod changelog_store; mod backend;` |
| `crates/shamir-engine/src/repo/journal/backend/mod.rs` | 1 | D1 | module_manifest | `mod durability_policy; mod group_commit; mod repo_types;` |
| `crates/shamir-engine/src/repo/seed/mod.rs` | 1 | D1 | module_manifest | `mod install; mod metadata; mod prepare_seed;` |
| `crates/shamir-engine/src/repo/tests/mod.rs` | 1 | D1 | module_manifest | `mod core; mod identity; mod journal; mod lifecycle;` |
| `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/recovery/mod.rs` | 1 | D1 | module_manifest | `mod clean_horizon_shutdown_impl; mod cold_reconcile_repo_impl;` |

## Baseline entries (generated)

| Kind | Path | Count | Stage |
|---|---|---:|---|
| dir | `crates/shamir-engine/src` | 12 | F |
| dir | `crates/shamir-engine/src/repo` | 19 | D1 |
| dir | `crates/shamir-engine/src/repo/tests` | 21 | D1 |
| file | `crates/shamir-engine/src/repo/repo_instance.rs` | 1967 | D1 |

## Anchored module mounts (generated)

| Source:line | attr |
|---|---|
| `crates/shamir-engine/src/repo/changelog_store.rs:16` | `#[path = "changelog_store/follower_durability.rs"]` |
| `crates/shamir-engine/src/repo/changelog_store.rs:19` | `#[path = "changelog_store/write_domain.rs"]` |
| `crates/shamir-engine/src/repo/changelog_store.rs:22` | `#[path = "changelog_store/write_domain_membership.rs"]` |
| `crates/shamir-engine/src/repo/repo_instance.rs:1` | `#[path = "durability_policy/durability_policy/repo_impl.rs"]` |
| `crates/shamir-engine/src/repo/repo_instance.rs:37` | `#[path = "clean_horizon/repo_impl.rs"]` |
| `crates/shamir-engine/src/repo/repo_instance.rs:40` | `#[path = "clean_horizon/shutdown_impl.rs"]` |
| `crates/shamir-engine/src/repo/repo_instance.rs:43` | `#[path = "cold_reconcile/repo_impl.rs"]` |
| `crates/shamir-engine/src/repo/repo_instance.rs:46` | `#[path = "changelog_store/repo_membership_impl.rs"]` |

## Restricted visibility in owned paths (generated)

| Source:line | declaration |
|---|---|
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/repo_impl.rs:187` | `pub(super) async fn read_changelog_replication_page(` |
| `crates/shamir-engine/src/repo/repo_instance.rs:58` | `pub(super) repo: BoxRepo,` |
| `crates/shamir-engine/src/repo/repo_instance.rs:177` | `pub(super) write_domain: Arc<RepoWriteDomain>,` |
| `crates/shamir-engine/src/repo/repo_instance.rs:1521` | `pub(super) async fn read_changelog_bounded_impl(` |
| `crates/shamir-engine/src/repo/seed_install/inventory.rs:42` | `pub(super) async fn save_inventory(store: &Arc<dyn Store>, tables: &[Self]) -> DbResult<()> {` |
| `crates/shamir-engine/src/repo/seed_install/receipt.rs:79` | `pub(super) async fn save(&self, store: &Arc<dyn Store>) -> DbResult<()> {` |
| `crates/shamir-engine/src/repo/seed_install/row.rs:12` | `pub(super) fn translate_row(bytes: Bytes, remap: &TFxMap<u64, u64>) -> DbResult<Bytes> {` |
| `crates/shamir-engine/src/repo/seed_install/tests/flush_probe.rs:16` | `pub(super) struct FlushProbe {` |
| `crates/shamir-engine/src/repo/seed_install/tests/helpers.rs:12` | `pub(super) fn receipt() -> SeedReceipt {` |
| `crates/shamir-engine/src/repo/seed_install/tests/helpers.rs:27` | `pub(super) fn repo(backing: Arc<InMemoryRepo>, tables: Vec<SeedTable>) -> RepoInstance {` |
| `crates/shamir-engine/src/repo/seed_install/tests/helpers.rs:35` | `pub(super) fn inventory() -> Vec<SeedTable> {` |
| `crates/shamir-engine/src/repo/seed_install/tests/helpers.rs:48` | `pub(super) fn payload() -> Bytes {` |
| `crates/shamir-engine/src/repo/seed_install/tests/helpers.rs:59` | `pub(super) async fn populated(repo: RepoInstance) -> SeedImageWriter {` |
| `crates/shamir-engine/src/repo/seed_install/writer.rs:37` | `pub(super) fail_at: Option<usize>,` |
| `crates/shamir-engine/src/repo/seed_install/writer.rs:39` | `pub(super) boundaries: Vec<usize>,` |
| `crates/shamir-engine/src/repo/seed_install/writer.rs:41` | `pub(super) bookmark_store: Option<Arc<dyn Store>>,` |

## RepoInstance impl sites in owned paths (generated)

| Source:line | declaration |
|---|---|
| `crates/shamir-engine/src/repo/changelog_store/follower_durability.rs:9` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/changelog_store/repo_membership_impl.rs:21` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/changelog_store/write_domain_membership.rs:9` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/clean_horizon/repo_impl.rs:12` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/clean_horizon/shutdown_impl.rs:60` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/cold_reconcile/repo_impl.rs:6` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/durability_policy/durability_policy/repo_impl.rs:10` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/repo_instance.rs:236` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/seed_install/backing.rs:10` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/seed_metadata.rs:6` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/visibility_observer.rs:12` | `impl RepoInstance {` |
| `crates/shamir-engine/src/repo/visibility_observer.rs:156` | `impl RepoInstance {` |
