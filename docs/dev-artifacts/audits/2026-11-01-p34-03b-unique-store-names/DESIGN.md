# P34-03b — unique physical store names: design and implementation review

**Status: PARTIAL — implementation and source review present; CI pending.** No compile/test/build command has been run. Formatting was run after final Rust edits. Layout gate remains incomplete because of the recorded folder-entry count violations.

- D legacy PARTIAL pending CI: `crates/shamir-db/src/shamir_db/tests/ddl_contracts/unique_store_names_tests.rs::legacy_catalogue_row_without_store_id_remains_idless` seeds exact legacy stores directly through fjall_raw, populates through a low-level `RepoInstance`, flushes fixture metadata, and separately saves an id-less builder-document catalogue record. The facade cold-opens, reads, renames, and cold-reopens while checking exact old physical store names. This verifies logical reopen behavior, not power-loss durability; no local compilation or tests were run.

- D copy failure regression PARTIAL: `crates/shamir-engine/src/repo/instance/repo_instance/table_store/tests/copy.rs::second_page_copy_failure_preserves_partial_write_and_retry_target_is_clean` injects a partial write then error on batch two, confirms 300 source values remain and a separate retry target holds the exact values. It exercises the same `copy_store_contents` helper used by rename, but does NOT inject through the full rename path, verify old logical name usability, or prove failed generated store IDs remain unused. Full rename second-copy-page failure remains outstanding. No build/test command was run.


Chosen rename variant (b): copy the three stores to a fresh destination ID. `TableConfig` has optional node-local `store_id`; absent IDs retain legacy names. The mapper in `crates/shamir-engine/src/repo/instance/repo_instance/table_store/names.rs` appends `--{RecordId}` to the data, info, and history names. Registration/open/copy/stage paths use it; logical names/tokens are unchanged. Dropped and failed-copy stores remain orphaned; purge is out of scope.

`TableConfig::new` remains id-less; ShamirDb CREATE assigns a fresh ID (`crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:73-88`). Catalogue metadata stores optional IDs; missing IDs preserve legacy behavior, malformed present values reject. No migration. Old binaries are not expected to open new ID-bearing data; rollback requires backup. `KNOWN_LIMITATIONS.md` records compatibility.

### D1 seed-local identity repair

`SeedTable` public fields and serialized inventory remain unchanged. The seed image `__tx__` store has a separate optional `__meta__/seed_install/local_table_ids` envelope containing ordered `(name, RecordId)` pairs. It is flushed before Complete; absent mapping means legacy id-less inventory. `SeedTable::load_configs` validates count/name correspondence. Public owned `RepoInstance::table_config(&str)` exposes the actual config for seed schema/catalogue use. ShamirDb stores `store_id` in its private catalogue record and validates the catalogue ID against local config. IDs are not added to exported `TableRecord` or schema digest. Imported tables keep their own ID and reopen using the local map.

Relevant code: `crates/shamir-engine/src/repo/seed/install/inventory.rs:12-13,29-54,87-122`; `writer.rs:295-306`; `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:251-253`; `crates/shamir-db/src/shamir_db/shamir_db/seed/schema.rs:81-92`; `seed/catalogue.rs:129-154`.

## Rename protocol and limits

The engine drains source MVCC and copies data/info/history to a fresh destination ID while source remains registered. `rename_table_stores_with` keeps one source Repo permit through prepare/drain/copy/persist/switch; ShamirDb persists through a separate system repository callback. The catalogue row retains metadata and adds private `rename_from` and optional `rename_from_store_id` provenance. These provenance fields remain necessary despite logical transactional persistence: a durability-tail error may return after destination catalogue persistence while live source registration is unchanged, and sequential backend transact visibility can leave both rows. Retry uses exact source/destination IDs to remove only a matching stale source row; recreated/unrelated rows are rejected. Missing source ID is valid legacy provenance. Boot can register both rows if recovery leaves both; no automatic cleanup is claimed.

This is not raw `Store::transact` visibility atomicity. Storage default transact applies ops sequentially; Fjall reports `supports_atomic_transact=false`. Destination stores flush before catalogue persistence; this is an API-level barrier, not verified crash durability. Runtime/crash behavior remains CI-unverified. `rename_membuffer_wrapped_table_flushes_copied_history` covers 300-row multi-page success, not injected copy-page failure/retry.

## Public API changes

- `TableConfig.store_id`, `TableConfig::with_new_store_id`, `TableConfig::with_store_id` (`crates/shamir-engine/src/table/support/model/table_config.rs`).
- `RepoInstance::table_config(&str) -> Option<TableConfig>` returns an owned config.
- `RepoInstance::rename_table_stores_with` and `recover_table_rename_with` (`crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:285-312`).
- `SeedTable::load_configs(&Arc<dyn Store>) -> DbResult<Vec<TableConfig>>` (`crates/shamir-engine/src/repo/seed/install/inventory.rs:31`). Existing `SeedTable` public fields/serialization are unchanged.
- Existing public `SystemStore::save_table` signature changed to accept optional ID (`crates/shamir-db/src/shamir_db/system_store/catalogue.rs:188+`).
- No public `DbInstance` wrapper or raw public commit helper added.

## Tests and evidence (source reviewed, not executed)

- Engine regression `disk_cold_reopen_preserves_receipt_counts_bindings_and_local_floor` (`crates/shamir-engine/src/repo/seed/install/tests/disk_tests.rs:9`) loads the local map, checks ID equality, suffixed stores/no unsuffixed history store, and normal row/counter reads after cold reopen. `cold_handles_read_remapped_rows_counters_and_bootstrap_gap` (`image_tests.rs:73`) also reopens from `load_configs` and checks IDs/row.
- ShamirDb `names_schema_is_compiled_and_durable_before_complete` (`crates/shamir-db/src/shamir_db/shamir_db/seed/tests/schema_tests.rs:15`) asserts the private schema record has local store ID.
- DDL isolation/retry/recovery regressions are recorded at `crates/shamir-db/src/shamir_db/shamir_db/tests/unique_store_names_tests.rs`; seed replication/fence regression at `crates/shamir-server/src/sync/replication/transport/seed/tests/loop_tests.rs:317+`.
- Server seed install regression `imported_seed_ids_are_node_local_persisted_and_digest_is_logical` in `crates/shamir-server/src/sync/replication_seed/install/tests/disk_tests.rs:65` verifies the artifact, installs it, cold-opens normal ShamirDb, checks inserted row values with `get_many`, compares source/import local catalogue IDs, renames/recreates to a different ID, exports/verifies again, and compares `manifest.schema_sha256` plus logical table names.

Gaps: full rename-path injected failure on second copy batch remains absent; helper-only partial-write/retry test is present. Legacy raw-name/id-less catalogue fixture now exists in source but is not runtime verified. Seed exporter/verifier/import/cold-read and digest source coverage exists, unexecuted. No local compilation/tests/clippy; CI pending.

## Scope / process notes

No storage crate changes or new dependencies. No files moved or deleted. Previous progress remains: C done; A/B is partial, including completed restoration of the original `disk_export_tests.rs` helpers/tests as earlier source verification confirms. Do not restore the broken source-replacement seed test verbatim from DESIGN's fenced text. Folder limits recorded as violated: `system_store` 9 direct entries and `ddl_contracts` 8, over max 7; no moves performed.

**E hygiene/audit progress:** Reviewed import/privacy/module/API/cfg and comments source-only, without compiler confirmation. Restored `Arc` import in engine `repo/seed/install/tests/validation_tests.rs`; restored original six-line writer allocation-evidence/audit comment block. Hoisted `IsolationLevel` in engine `repo/tests/core/repo_instance_tests.rs`; table-store test manifest has `#[cfg(test)] mod tests;` with names/record_id declarations. Existing comments remain; no claim compile/lint passed. Formatter line wrapping retains disk-export comment text. Source catalogue-loading docs were checked against HEAD and unchanged.

**E verified constraints:** FOUR direct-entry count violations: `crates/shamir-db/src/shamir_db/shamir_db/tests` has 8, `crates/shamir-db/src/shamir_db/system_store` has 9, `crates/shamir-db/src/shamir_db/tests/ddl_contracts` has 8, and `crates/shamir-engine/src/repo/instance/repo_instance` has 8 (limit 7). No move/delete was made; orchestrator regrouping required. New table_store folder has 5 entries; tests subfolder has 3. New source files are within 1000 lines. Automated layout script does not count these manual/untracked entries.

**D2 progress: PARTIAL pending CI.** Test paths are conventionally wired; source review complete; execution remains CI-only. Store-name mapping and fresh identity behavior are covered by engine tests, DB DDL regressions, and seed import/fence tests. No local compile/test/build/clippy was run.


## Final report

Overall PARTIAL; see corrected A–G status and evidence below. A — PARTIAL: imports/privacy/module/API/cfg source review and fixes, no compiler/clippy confirmation. B — DONE source wiring: `replication_ddl_tests` restored in `crates/shamir-db/src/shamir_db/tests/ddl_contracts/mod.rs:6`; original three disk-export tests at `crates/shamir-server/src/sync/replication_seed/tests/disk_export_tests.rs:267,272,282`. C — PARTIAL: rename transaction/provenance retry in `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:285,302`, `crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:359,403`, `crates/shamir-db/src/shamir_db/system_store/catalogue.rs:251`; no runtime crash verification, and source writes during tail-error/retry are untested. D — PARTIAL: data-bearing drop/swap, legacy fixture, persistence/recovery and seed tests present, all unrun; full rename second-batch fault injection missing (helper-only). E — PARTIAL: four folder-count violations listed above; orchestrator must regroup. F — PARTIAL: fmt, fmt --check, layout script, and orchestrator's diff check each exit 0 (DIFF_CHECK_EXIT0 observed at 16:21 CET after Rust edits); compilation/clippy/tests/CI NOT RUN. G — DONE for documentation only; no runtime assertion implied.

Tests (source-reviewed, not run): `drop_recreate_separates_store_incarnations`; `rename_recreate_and_swap_names_preserves_each_incarnation`; `durability_tail_failure_retry_reconciles_registration_and_is_idempotent`; `rename_drop_recreate_uses_fresh_store_incarnation`; `legacy_catalogue_row_without_store_id_remains_idless`; `second_page_copy_failure_preserves_partial_write_and_retry_target_is_clean` (helper only, no full rename fault injection); `generated_table_store_id_survives_drop_and_recreate`; `generated_table_store_id_survives_lazy_open_and_legacy_config_stays_legacy`; `rename_membuffer_wrapped_table_flushes_copied_history`; `disk_cold_reopen_preserves_receipt_counts_bindings_and_local_floor`; `cold_handles_read_remapped_rows_counters_and_bootstrap_gap`; `optional_local_mapping_accepts_legacy_and_rejects_corrupt_maps`; `precommit_persist_failure_retry_uses_fresh_destination_id`; `retry_removes_stale_source_catalogue_row_only`; `legacy_idless_stale_source_row_is_removed_on_retry`; `retry_rejects_recreated_source_and_preserves_both_incarnations`; `retry_rejects_unrelated_destination_without_changing_rows`; `legacy_row_stays_idless_and_malformed_id_is_rejected`; `names_schema_is_compiled_and_durable_before_complete`; `imported_seed_ids_are_node_local_persisted_and_digest_is_logical`. All source-reviewed only; no runtime confirmation. Full rename second-copy-batch injected-failure test is absent.

Public APIs intended: `TableConfig.store_id`, `with_new_store_id`, `with_store_id`; `RepoInstance.rename_table_stores_with`, `recover_table_rename_with`, `table_config`; `SeedTable.load_configs`; changed `SystemStore.save_table` signature only.

## Changed/untracked Rust source read end-to-end (40 files)

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

No compile/build/test/clippy/CI gate passed or was run. Orchestrator must remove accidental untracked root `DESIGN.md` and empty `docs/dev-artifacts/audits/2026-11-01-p34-03b-unique-store-names/docs-placeholder`; this agent did not delete/move either. Four over-cap folders require regrouping. Destination IDs may orphan on failure; destination flush before catalogue persistence is not raw-store atomic. Boot may reopen source if both rows persisted; retry the same rename. Writes between durability-tail error and retry are untested; no safety claim.Tests (source-reviewed, not run): `drop_recreate_separates_store_incarnations`; `rename_recreate_and_swap_names_preserves_each_incarnation`; `durability_tail_failure_retry_reconciles_registration_and_is_idempotent`; `rename_drop_recreate_uses_fresh_store_incarnation`; `legacy_catalogue_row_without_store_id_remains_idless`; `second_page_copy_failure_preserves_partial_write_and_retry_target_is_clean` (helper only, no full rename fault injection); `generated_table_store_id_survives_drop_and_recreate`; `generated_table_store_id_survives_lazy_open_and_legacy_config_stays_legacy`; `rename_membuffer_wrapped_table_flushes_copied_history`; `disk_cold_reopen_preserves_receipt_counts_bindings_and_local_floor`; `cold_handles_read_remapped_rows_counters_and_bootstrap_gap`; `optional_local_mapping_accepts_legacy_and_rejects_corrupt_maps`; `precommit_persist_failure_retry_uses_fresh_destination_id`; `retry_removes_stale_source_catalogue_row_only`; `legacy_idless_stale_source_row_is_removed_on_retry`; `retry_rejects_recreated_source_and_preserves_both_incarnations`; `retry_rejects_unrelated_destination_without_changing_rows`; `legacy_row_stays_idless_and_malformed_id_is_rejected`; `names_schema_is_compiled_and_durable_before_complete`; `imported_seed_ids_are_node_local_persisted_and_digest_is_logical`. All source-reviewed only; no runtime confirmation. Full rename second-copy-batch injected-failure test is absent.

The exact 40 changed/untracked Rust paths read end-to-end are listed one per line in `FINAL_REPORT.md`. Source fixture review is not runtime verification. No compile/build/test/clippy/CI gate passed or was run. Orchestrator must remove accidental untracked root `DESIGN.md` (six progress lines) and empty `docs/dev-artifacts/audits/2026-11-01-p34-03b-unique-store-names/docs-placeholder`; this agent did not delete/move either. Four over-cap folders require regrouping. Destination IDs may orphan on failure; destination flush before catalogue persistence is not raw-store atomic. Boot can reopen source if both rows persisted; retry the same rename. Writes between durability-tail error and retry are untested; no safety claim.
