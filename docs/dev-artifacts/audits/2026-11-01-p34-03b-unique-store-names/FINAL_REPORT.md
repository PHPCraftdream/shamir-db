## Final report

**Overall status: PARTIAL.** Source audit complete; compile and runtime verification remain CI-only.

### A–G status (per task definitions)

- **A — PARTIAL (compiler/privacy/import review):** Reviewed imports, visibility/privacy, module access, API calls, signatures, Arc coercions, cfg, and deliberate trait/lifetime/Send/exhaustiveness concerns. Relevant source hygiene/import fixes are present, including restored `Arc` import in engine seed validation and retained writer audit comments. No local compilation/clippy or CI confirms these; no compiler pass is claimed.
- **B — DONE (test wiring/source):** `replication_ddl_tests` is restored at `crates/shamir-db/src/shamir_db/tests/ddl_contracts/mod.rs:6`; the three original disk-export tests are present in `crates/shamir-server/src/sync/replication_seed/tests/disk_export_tests.rs:267,272,282`. Source-verified only, not executed.
- **C — PARTIAL (rename recovery):** Transaction/provenance retry source exists: engine `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:285,302`; DB `crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:359,403`; system catalogue `crates/shamir-db/src/shamir_db/system_store/catalogue.rs:251`. Runtime/crash recovery is unverified. Source writes between durability-tail error and retry are untested; no safety claim.
- **D — PARTIAL (data-bearing tests):** Normal drop/recreate/swap, legacy raw-name/id-less catalogue, persistence/recovery and seed import/reopen/local-map tests exist. Full rename-path injected second-copy-batch fault test is absent; copy helper test alone does not prove rename recovery.
- **E — PARTIAL (structure/comments):** Four manual directory-count violations: `crates/shamir-db/src/shamir_db/shamir_db/tests` (8), `crates/shamir-db/src/shamir_db/system_store` (9), `crates/shamir-db/src/shamir_db/tests/ddl_contracts` (8), `crates/shamir-engine/src/repo/instance/repo_instance` (8). No files moved/deleted; orchestrator regrouping required. All changed Rust source files are <=995 lines. Restored Arc import and six writer comments; comments retained. Automated layout script omits untracked/manual counts.
- **F — PARTIAL (gates):** Formatting exit 0; formatting check exit 0; `python -I scripts/ci/check_layout.py` exit 0. Orchestrator also observed `DIFF_CHECK_EXIT0` after final Rust edits at 16:21 CET. Compilation, clippy, tests, and CI are NOT RUN.
- **G — DONE (documentation only):** This design document and report record the implementation status, limits, APIs, tests, exact source read list, and follow-up. This does not imply runtime verification.

### Tests added or strengthened (source-reviewed; not run)

- `drop_recreate_separates_store_incarnations`, `rename_recreate_and_swap_names_preserves_each_incarnation`, and `durability_tail_failure_retry_reconciles_registration_and_is_idempotent` in `crates/shamir-db/src/shamir_db/shamir_db/tests/unique_store_names_tests.rs`.
- `rename_drop_recreate_uses_fresh_store_incarnation` and `legacy_catalogue_row_without_store_id_remains_idless` in `crates/shamir-db/src/shamir_db/tests/ddl_contracts/unique_store_names_tests.rs`.
- Engine: `second_page_copy_failure_preserves_partial_write_and_retry_target_is_clean` (helper-only, not full rename injection); `generated_table_store_id_survives_drop_and_recreate`; `generated_table_store_id_survives_lazy_open_and_legacy_config_stays_legacy`; `rename_membuffer_wrapped_table_flushes_copied_history`.
- Seed/recovery/schema: `disk_cold_reopen_preserves_receipt_counts_bindings_and_local_floor`; `cold_handles_read_remapped_rows_counters_and_bootstrap_gap`; `optional_local_mapping_accepts_legacy_and_rejects_corrupt_maps`; `precommit_persist_failure_retry_uses_fresh_destination_id`; `retry_removes_stale_source_catalogue_row_only`; `legacy_idless_stale_source_row_is_removed_on_retry`; `retry_rejects_recreated_source_and_preserves_both_incarnations`; `retry_rejects_unrelated_destination_without_changing_rows`; `legacy_row_stays_idless_and_malformed_id_is_rejected`; `names_schema_is_compiled_and_durable_before_complete`.
- Server: `imported_seed_ids_are_node_local_persisted_and_digest_is_logical` plus fence/disk-export regressions. All listed tests are source-reviewed, not run; full rename second-copy-batch injected-failure test is absent.

### Public API changes

`TableConfig.store_id`, `TableConfig::with_new_store_id`, `TableConfig::with_store_id`; `RepoInstance::rename_table_stores_with`, `RepoInstance::recover_table_rename_with`, `RepoInstance::table_config`; `SeedTable::load_configs`; changed existing `SystemStore::save_table` signature. No other external public item intended.

### Changed/untracked Rust source read end-to-end (40 files)

- `crates/shamir-db/src/shamir_db/shamir_db/core.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/seed/catalogue.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/seed/schema.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/seed/tests/schema_tests.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/tests/mod.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/tests/unique_store_names_tests.rs`
- `crates/shamir-db/src/shamir_db/system_store/catalogue.rs`
- `crates/shamir-db/src/shamir_db/system_store/mod.rs`
- `crates/shamir-db/src/shamir_db/system_store/rename_recovery.rs`
- `crates/shamir-db/src/shamir_db/system_store/table_catalogue_config.rs`
- `crates/shamir-db/src/shamir_db/tests/ddl_contracts/mod.rs`
- `crates/shamir-db/src/shamir_db/tests/ddl_contracts/unique_store_names_tests.rs`
- `crates/shamir-engine/src/db_instance/db_instance.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/copy.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/mod.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/names.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/record_id.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/tests/copy.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/tests/mod.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/tests/names.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/table_store/tests/record_id.rs`
- `crates/shamir-engine/src/repo/instance/repo_instance/transactions/write_domain_membership.rs`
- `crates/shamir-engine/src/repo/seed/install/inventory.rs`
- `crates/shamir-engine/src/repo/seed/install/tests/disk_tests.rs`
- `crates/shamir-engine/src/repo/seed/install/tests/durability_tests.rs`
- `crates/shamir-engine/src/repo/seed/install/tests/image_tests.rs`
- `crates/shamir-engine/src/repo/seed/install/tests/validation_tests.rs`
- `crates/shamir-engine/src/repo/seed/install/writer.rs`
- `crates/shamir-engine/src/repo/tests/core/repo_instance_tests.rs`
- `crates/shamir-engine/src/table/support/model/table_config.rs`
- `crates/shamir-engine/src/tx/replication/follower_durability/receipt/stage.rs`
- `crates/shamir-server/src/sync/replication/transport/seed/fence.rs`
- `crates/shamir-server/src/sync/replication/transport/seed/tests/loop_tests.rs`
- `crates/shamir-server/src/sync/replication_seed/install/tests/disk_tests.rs`
- `crates/shamir-server/src/sync/replication_seed/tests/disk_export_tests.rs`

### Required follow-up and limitations

Orchestrator must remove accidental untracked root `DESIGN.md` (six progress lines) and empty `docs/dev-artifacts/audits/2026-11-01-p34-03b-unique-store-names/docs-placeholder`; this agent did not delete either. Orchestrator must regroup the four over-cap folders and rerun layout. Fresh destination IDs can orphan on failure; destination flush precedes catalogue persistence, which is not raw-store atomic. Boot may reopen source if both rows persist; operator retry the same rename. Source writes between durability-tail error and retry are untested; no safety claim. All compile/build/test/clippy/CI gates remain NOT RUN.
