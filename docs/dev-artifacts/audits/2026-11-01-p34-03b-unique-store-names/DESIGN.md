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

Durability-tail safety remains PARTIAL in this revision: source write fencing and exact-pair recovery recopy were not implemented. Default `Repo::copy_store` merge/upsert semantics overwrite matching keys but retain destination-only keys and may leave partial writes on failure; a safe convergence argument must account for deletions/tombstones/history metadata and all direct/stale handle write paths before claiming recovery. Until then, a persist error may leave source writes diverging from the committed destination; CI cannot supply the missing design proof.

`TableConfig::validate_store_name` is the public crate API that bridges DB callers to the private engine mapper; only the error/result surface is widened by this requested functionality. It validates generated names and leaves id-less legacy configs unchanged.

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

**D legacy fixture repair:** Source inspection shows the previous fixture used a second `RepoInstance` over the facade repo's shared backing, with an independent transaction gate; facade `flush_all_for_test` then flushed the original (empty) gate. I infer this could persist/reset shared checkpoint state to zero, so this repair registers the id-less `TableConfig` through the facade's admitted write domain and uses the facade owner for writes and draining/flushing. This is fixture repair only—not evidence that production data visibility is fixed. CI execution remains required.

**E hygiene/audit progress:** Reviewed import/privacy/module/API/cfg and comments source-only, without compiler confirmation. Restored `Arc` import in engine `repo/seed/install/tests/validation_tests.rs`; restored original six-line writer allocation-evidence/audit comment block. Hoisted `IsolationLevel` in engine `repo/tests/core/repo_instance_tests.rs`; table-store test manifest has `#[cfg(test)] mod tests;` with names/record_id declarations. Existing comments remain; no claim compile/lint passed. Formatter line wrapping retains disk-export comment text. Source catalogue-loading docs were checked against HEAD and unchanged.

**E verified constraints:** FOUR direct-entry count violations: `crates/shamir-db/src/shamir_db/shamir_db/tests` has 8, `crates/shamir-db/src/shamir_db/system_store` has 9, `crates/shamir-db/src/shamir_db/tests/ddl_contracts` has 8, and `crates/shamir-engine/src/repo/instance/repo_instance` has 8 (limit 7). No move/delete was made; orchestrator regrouping required. New table_store folder has 5 entries; tests subfolder has 3. New source files are within 1000 lines. Automated layout script does not count these manual/untracked entries.

**D2 progress: PARTIAL pending CI.** Test paths are conventionally wired; source review complete; execution remains CI-only. Store-name mapping and fresh identity behavior are covered by engine tests, DB DDL regressions, and seed import/fence tests. No local compile/test/build/clippy was run.



## Revision 3 progress

Item 2 source review: the legacy floor concern is not attributed to missing constructor recovery. The fixture now drops the write-domain permit before table access and avoids stopping the journal writer; persistence behavior is not claimed without runtime verification. `RepoInstance::checkpoint_before_truncate` skips mem WAL while the disk-backed facade uses persistent WAL checkpointing, so the observed fixture discrepancy alone does not establish a constructor-recovery defect.

Item 3 implemented at `crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs`: `add_table_as_locked` reuses `repo.table_config(table_name)` when present, preserving its store identity and index setting; only new registration gets an ID. Catalogue save uses the actual config values, and a catalogue save error drops only newly registered membership, preserving pre-existing registrations. Source evidence: this path previously always generated an ID at former lines 70-76 and unconditionally dropped registration on save failure at former lines 94-95. No runtime/compile validation was run; CI must verify item 2 and failure3 regressions.



## Final report

1. PARTIAL — durability-tail fencing/recovery not implemented. `Repo::copy_store` merge/upsert retains destination-only keys and may partially write on failure, so a full convergence proof requires handling destination-only data, tombstones/history metadata, and every direct/stale handle admission path. Existing `durability_tail_failure_retry_reconciles_registration_and_is_idempotent` remains unchanged and has no intervening write assertion.
2. DONE in source — inline tables receive IDs before catalogue capture in `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:431-441`; CREATE REPO TABLES assigns IDs at `crates/shamir-db/src/shamir_db/execute/admin_db/admin_db_repo.rs:413-418`. Legacy fixtures explicitly persist id-less catalogue rows in `crates/shamir-db/src/shamir_db/shamir_db/tests/catalogue/unique_store_names_tests.rs:462-550`. Regressions: `inline_repo_tables_receive_store_ids`, `legacy_inline_catalogue_rows_remain_idless`, `create_repo_via_execute_persists_to_catalogue`; source-only, CI pending.
3. DONE in source — `TableConfig::validate_store_name` encapsulates the engine-private mapper and is called by normal CREATE, inline `add_repo`, wire CREATE REPO TABLES before persistence, and engine RENAME. The fixed-ID API boundary test uses `[0xff; 16]` and asserts exactly-fit/one-byte-over; normal CREATE and inline CREATE rejection tests use 255-byte names (definitely overlong independent of random IDs). Rename has no dedicated rejection regression, so that call path is source-reviewed only. Evidence: `crates/shamir-engine/src/table/support/model/table_config.rs:35-52`; `crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:74-76`; `db_management.rs:432-438`; `crates/shamir-db/src/shamir-db/execute/admin_db/admin_db_repo.rs:413-418`; `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:472-480`; `crates/shamir-engine/src/repo/instance/repo_instance/table_store/tests/names.rs:9-21`; DB regressions in `.../unique_store_names_tests.rs:433-493`. No tests/compile run; CI pending.
4. PARTIAL — lint-aware read-through was incomplete, especially all direct/stale-handle paths and all surrounding changed references. Required files read include `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs`, `crates/shamir-engine/src/repo/instance/repo_instance/table_store/names.rs`, `crates/shamir-engine/src/repo/instance/repo_instance/table_store/tests/names.rs`, `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs`, `crates/shamir-db/src/shamir-db/execute/admin_db/admin_db_repo.rs`, `crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs`, `crates/shamir-db/src/shamir_db/shamir_db/tests/catalogue/unique_store_names_tests.rs`, `crates/shamir-db/src/shamir_db/tests/core/execute_tests.rs`, `crates/shamir-engine/src/db_instance/db_instance.rs`, `crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs`, `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs`, `crates/shamir-engine/src/repo/instance/repo_instance/table_store/copy.rs`, `crates/shamir-storage/src/core/types.rs`, and `crates/shamir-storage/src/core/error.rs`. Restored malformed in-progress files from clean `2a4c2f436`; recovery snapshots were read. No compile/clippy/test/build run. Scoped format and layout gates must be run; CI must type-check and execute tests.

Public APIs intended: `TableConfig.store_id`, `with_new_store_id`, `with_store_id`; `RepoInstance.rename_table_stores_with`, `recover_table_rename_with`, `table_config`; `SeedTable.load_configs`; changed `SystemStore.save_table` signature only.

Writes between durability-tail error and retry are untested; no safety claim.

## Item 4 audit

Item 4 fixture correction: source inspection showed `export_open` validates user record payloads before the subsequent test call to `load_table_record`: `crates/shamir-server/src/sync/replication_seed/export/exporter.rs:221` calls the record validator, and `crates/shamir-server/src/sync/replication_seed/artifact/encoding/codec.rs:48` rejects a non-map payload. The new fixture in `crates/shamir-server/src/sync/replication_seed/install/tests/disk_tests.rs` supplied root `InnerValue::Str` rows, so export failed validation before the catalogue loader was reached; this is not evidence of a production catalogue encoding mismatch. The fixture now inserts document payloads via `doc!` and `query_value_to_inner`, using the source table interner, then after cold install checks exact decoded documents with `inner_value_to_query_value` and the destination repo interner. Store-ID, key, node-local ID, schema-digest, row-count, and other value assertions remain intact. Production codec strictness is unchanged. No compile/runtime test was run; CI must verify the regression.

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

Item 1 source fix: `rename_table_as_locked` now obtains one namespace catalogue snapshot and derives the source and destination rows from it, reusing that snapshot for foreign-key validation; `rename_table_row` retains its separate source-incarnation revalidation. Thus the normal rename catalogue reads are the snapshot plus the pre-commit source check (two); recovery validation in `complete_table_rename_row` remains unchanged. The existing `rv_db_72e8717c_rename_reads_table_catalogue_once_for_many_siblings` test and its expected count are unchanged. Source reviewed only; no runtime/compile claim, CI verification pending.

Item 2 evidence correction (inference from source, not runtime-proven): the original fixture's low-level `RepoInstance::new` had an in-memory WAL and independent gate, while the facade had no registered old table; fixture direct history writes lacked persisted last-committed/J floor, and final facade shutdown checkpointed its empty gate. Reusing the recovered facade ensures clean shutdown checkpoints the writing gate. See `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/durability_repo_impl.rs:244-261` and `crates/shamir-engine/src/repo/instance/repo_instance/transactions.rs:56-74`.

P34-03b progress: `add_table_preserves_pre_registered_config_and_rows` now exercises generated and legacy pre-registered tables across a synchronous runtime teardown and cold Fjall reopen, asserting catalogue/config identity, index setting, IDs, and exact rows. Source reviewed; CI tests pending.

## Revision 3 final report

Status: IMPLEMENTED IN SOURCE, CI PENDING; none of the reported regressions is runtime-verified. The source mechanisms below are observed; their correspondence to reported CI failures is inferred, not established by local execution.

1. **Rename catalogue reads.** Observed: `rename_table_as_locked` reuses one namespace snapshot for source/destination and FK validation (`crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:288-290,411`); the separate source recheck remains (`crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:296-310`). Normal static call accounting is two reads. The existing read-count test at `crates/shamir-db/src/shamir_db/tests/core/ddl_namespace_lock_tests.rs:203` still expects two; oracle unchanged. CI must confirm.
2. **Legacy id-less fixture.** Observed: the original fixture used a second `RepoInstance::new` over facade backing; its constructor uses mem WAL (`crates/shamir-engine/src/repo/instance/repo_instance.rs:227-233`). Direct `insert_many` writes history versions but no persistent last-committed marker (`crates/shamir-tx/src/mvcc_store/store.rs:807-893`). The facade initially had no old registration/gate progress; current stream bounds use `gate.last_committed` (`crates/shamir-tx/src/mvcc_store/history/version_entry.rs:486-491`), and boot floor uses marker/max-inflight/J (`crates/shamir-engine/src/repo/instance/repo_instance/transactions.rs:56-74`). Inference: a missing boot floor could hide rows; reset is not established, and this source mechanism is not a runtime-proven CI root cause. Fixture now uses one recovered facade owner, id-less low-level `add_table_admitted`, and permit drop (`crates/shamir-db/src/shamir_db/tests/ddl_contracts/unique_store_names_tests.rs:218-235`), then clean shutdown checkpoints the gate (`:253`; `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/durability_repo_impl.rs:244-261`). Original two-row/raw-name/no-ID/rename/cold-reopen assertions remain. Not a production name migration. CI needed for the legacy named test on Ubuntu and Windows.
3. **Seed recovery registration.** Observed: `SeedImageWriter` minted ID A (`crates/shamir-engine/src/repo/seed/install/writer.rs:143-151`); recovery helper calls `shamir.add_table` after seed (`crates/shamir-db/src/shamir_db/execute/replication_seed/tests/quarantine_reads_tests.rs:650-656`); old `add_table` minted B while the engine retained handle A (`crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs:51-53,298-304`). Observed fix: reuse actual existing `TableConfig`, preserving `store_id`/indexes, and rollback only a newly registered table (`crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:70-106`). Correspondence to reported CI failure is inferred pending CI. Cold regression at `crates/shamir-db/src/shamir_db/tests/ddl_contracts/unique_store_names_tests.rs:383-498` covers generated/idless configs, indexes, exact rows; save-failure regression at `:501-545`. Existing `binding_scope_resume_marker_removal_survives_reopen` unchanged. CI needed.
4. **Server artifact fixture.** Observed: exact rejection is for non-map USER payloads (`crates/shamir-server/src/sync/replication_seed/artifact/encoding/codec.rs:41-49`), called by exporter (`crates/shamir-server/src/sync/replication_seed/export/exporter.rs:221`) before the later catalogue read. Old `artifact_with_source_id` supplied scalar payloads, so export failed before catalogue test read. No catalogue codec corruption was observed; origin/master catalogue save/load formats match. Fixture now uses map documents and source-interner encoding (`crates/shamir-server/src/sync/replication_seed/install/tests/disk_tests.rs:78-90`) and destination-interner decoding with exact document checks (`:133-141`). ID/key/row-count/digest assertions retained; strict root-map rejection unchanged (`crates/shamir-server/src/sync/replication_seed/tests/artifact_tests.rs:156-190`). Reported CI correspondence remains inferred. CI needed for named server import test on Unix.

Verification observed: exact `cargo fmt -p shamir-db -p shamir-engine -p shamir-server` and `-- --check` ran; `python -I scripts/ci/check_layout.py` passed (0 baseline, 0 hints); `git diff --check` passed. No compile/build/test/check/clippy/rustc/npm was executed, no CI triggered, no Cargo edits/git mutation/new files/moves/deletions. All three changed Rust files were read end-to-end lint-aware; imports/signatures opened, cfg(test)/test-util checked; no signatures, struct layouts, or enums changed, no workspace literal migration, new allows/local uses/inline tests, or file/folder limit changes (Rust <=1000 lines, folders <=7). Source final review found no concrete issue but cannot substitute compile/clippy/runtime. Legacy diagnosis is source inference until CI. Durability-tail fencing/recovery gaps remain out of scope and unchanged; no claim they are fixed.

Implemented revision 3 source changes across table_management.rs, DB unique_store_names_tests.rs, server disk_tests.rs, and DESIGN.md. Preserved existing table identities, removed redundant rename lookups, corrected legacy/seed fixtures, added cold-reopen and rollback regressions. fmt/layout/diff checks passed; no local compilation or tests. All four reported regressions require CI confirmation; rename fencing remains out of scope.
