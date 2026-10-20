# Engine sweep manifest — table subtree (Stage T)

Status: DECIDED proposal; source and baseline facts below are OBSERVED. No source moved and no compilation/discovery was performed.

## Scope and baseline inventory

The committed layout baseline has **38** direct entries under `src/table`, **114** under `src/table/tests`, and seven oversized source files within this subtree; both directory rows are included. Baseline rows below are read from `scripts/ci/layout_baseline.txt`; source mappings from `git ls-files` and line counts from working source.

| Kind | Path | Count | Stage |
|---|---|---:|---|
| dir | `crates/shamir-engine/src/table/tests` | 114 | T |
| file | `crates/shamir-engine/src/table/read_exec.rs` | 3092 | T |
| file | `crates/shamir-engine/src/table/table_manager.rs` | 1536 | T |
| file | `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 1552 | T |
| file | `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | 1107 | T |
| file | `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | 1155 | T |
| file | `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | 1009 | T |
| file | `crates/shamir-engine/src/table/write_exec.rs` | 1452 | T |
| dir | `crates/shamir-engine/src/table` | 38 | T |

## Source-to-destination mappings (OBSERVED inputs; DECIDED destinations)

All tracked files in `crates/shamir-engine/src/table/**` occur exactly once below. `lines` is projected primary output size for split sources, otherwise observed source size. Moved tests stay with their owning table topic at Stage T; the table subtree remains under `src/table`.

| Source | Destination | Lines | Logical old | Logical new | Stage |
|---|---|---:|---|---|---|
| `crates/shamir-engine/src/table/README.md` | `crates/shamir-engine/src/table/support/model/README.md` | 125 | `crate::table::README.md` | `documentation::support::README.md` | `T` |
| `crates/shamir-engine/src/table/buffer_config.rs` | `crates/shamir-engine/src/table/support/model/buffer_config.rs` | 63 | `crate::table::buffer_config` | `crate::table::support::model::buffer_config` | `T` |
| `crates/shamir-engine/src/table/candidate_set.rs` | `crates/shamir-engine/src/table/read/execution/candidate_set.rs` | 33 | `crate::table::candidate_set` | `crate::table::read::execution::candidate_set` | `T` |
| `crates/shamir-engine/src/table/degraded_index_count.rs` | `crates/shamir-engine/src/table/table_manager/diagnostics/degraded_index_count.rs` | 95 | `crate::table::degraded_index_count` | `crate::table::table_manager::diagnostics::degraded_index_count` | `T` |
| `crates/shamir-engine/src/table/doctor.rs` | `crates/shamir-engine/src/table/table_manager/diagnostics/doctor.rs` | 793 | `crate::table::doctor` | `crate::table::table_manager::diagnostics::doctor` | `T` |
| `crates/shamir-engine/src/table/filtered_vector.rs` | `crates/shamir-engine/src/table/read/vector/filtered_vector.rs` | 150 | `crate::table::filtered_vector` | `crate::table::read::vector::filtered_vector` | `T` |
| `crates/shamir-engine/src/table/in_flight_create_guard.rs` | `crates/shamir-engine/src/table/support/guards/in_flight_create_guard.rs` | 191 | `crate::table::in_flight_create_guard` | `crate::table::support::guards::in_flight_create_guard` | `T` |
| `crates/shamir-engine/src/table/index2_backfill_hook.rs` | `crates/shamir-engine/src/table/support/test_seams/index2_backfill_hook.rs` | 57 | `crate::table::index2_backfill_hook` | `crate::table::support::test_seams::index2_backfill_hook` | `T` |
| `crates/shamir-engine/src/table/interner_manager.rs` | `crates/shamir-engine/src/table/support/storage/interner_manager.rs` | 379 | `crate::table::interner_manager` | `crate::table::support::storage::interner_manager` | `T` |
| `crates/shamir-engine/src/table/migration_freeze_guard.rs` | `crates/shamir-engine/src/table/support/guards/migration_freeze_guard.rs` | 28 | `crate::table::migration_freeze_guard` | `crate::table::support::guards::migration_freeze_guard` | `T` |
| `crates/shamir-engine/src/table/mod.rs` | `crates/shamir-engine/src/table/mod.rs` | 72 | `crate::table::mod` | `crate::table` | `T` |
| `crates/shamir-engine/src/table/persistable.rs` | `crates/shamir-engine/src/table/support/storage/persistable.rs` | 70 | `crate::table::persistable` | `crate::table::support::storage::persistable` | `T` |
| `crates/shamir-engine/src/table/read_asof_seek.rs` | `crates/shamir-engine/src/table/read/planning/read_asof_seek.rs` | 365 | `crate::table::read_asof_seek` | `crate::table::read::planning::read_asof_seek` | `T` |
| `crates/shamir-engine/src/table/read_exec.rs` | `crates/shamir-engine/src/table/read/execution/dispatch.rs` | 853 | `crate::table::read_exec` | `crate::table::read::execution::dispatch` | `T` |
| `crates/shamir-engine/src/table/read_index_scan.rs` | `crates/shamir-engine/src/table/read/planning/read_index_scan.rs` | 855 | `crate::table::read_index_scan` | `crate::table::read::planning::read_index_scan` | `T` |
| `crates/shamir-engine/src/table/read_planner.rs` | `crates/shamir-engine/src/table/read/planning/read_planner.rs` | 617 | `crate::table::read_planner` | `crate::table::read::planning::read_planner` | `T` |
| `crates/shamir-engine/src/table/read_temporal.rs` | `crates/shamir-engine/src/table/read/planning/read_temporal.rs` | 664 | `crate::table::read_temporal` | `crate::table::read::planning::read_temporal` | `T` |
| `crates/shamir-engine/src/table/record_counter.rs` | `crates/shamir-engine/src/table/support/storage/record_counter.rs` | 347 | `crate::table::record_counter` | `crate::table::support::storage::record_counter` | `T` |
| `crates/shamir-engine/src/table/record_cow.rs` | `crates/shamir-engine/src/table/support/storage/record_cow.rs` | 41 | `crate::table::record_cow` | `crate::table::support::storage::record_cow` | `T` |
| `crates/shamir-engine/src/table/table.rs` | `crates/shamir-engine/src/table/support/model/table.rs` | 332 | `crate::table::table` | `crate::table::support::model::table` | `T` |
| `crates/shamir-engine/src/table/table_config.rs` | `crates/shamir-engine/src/table/support/model/table_config.rs` | 19 | `crate::table::table_config` | `crate::table::support::model::table_config` | `T` |
| `crates/shamir-engine/src/table/table_manager.rs` | `crates/shamir-engine/src/table/table_manager.rs` | 703 | `crate::table::table_manager` | `crate::table::table_manager` | `T` |
| `crates/shamir-engine/src/table/table_manager_buffer.rs` | `crates/shamir-engine/src/table/table_manager/operations/table_manager_buffer.rs` | 66 | `crate::table::table_manager_buffer` | `crate::table::table_manager::operations::table_manager_buffer` | `T` |
| `crates/shamir-engine/src/table/table_manager_changefeed.rs` | `crates/shamir-engine/src/table/table_manager/accessors/table_manager_changefeed.rs` | 71 | `crate::table::table_manager_changefeed` | `crate::table::table_manager::accessors::table_manager_changefeed` | `T` |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | `crates/shamir-engine/src/table/table_manager/operations/table_manager_crud.rs` | 869 | `crate::table::table_manager_crud` | `crate::table::table_manager::operations::table_manager_crud` | `T` |
| `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` | `crates/shamir-engine/src/table/table_manager/operations/table_manager_index_mgmt.rs` | 997 | `crate::table::table_manager_index_mgmt` | `crate::table::table_manager::operations::table_manager_index_mgmt` | `T` |
| `crates/shamir-engine/src/table/table_manager_locks.rs` | `crates/shamir-engine/src/table/table_manager/accessors/table_manager_locks.rs` | 65 | `crate::table::table_manager_locks` | `crate::table::table_manager::accessors::table_manager_locks` | `T` |
| `crates/shamir-engine/src/table/table_manager_replication.rs` | `crates/shamir-engine/src/table/table_manager/operations/table_manager_replication.rs` | 265 | `crate::table::table_manager_replication` | `crate::table::table_manager::operations::table_manager_replication` | `T` |
| `crates/shamir-engine/src/table/table_manager_sorted_index.rs` | `crates/shamir-engine/src/table/table_manager/operations/table_manager_sorted_index.rs` | 390 | `crate::table::table_manager_sorted_index` | `crate::table::table_manager::operations::table_manager_sorted_index` | `T` |
| `crates/shamir-engine/src/table/table_manager_streaming.rs` | `crates/shamir-engine/src/table/table_manager/accessors/table_manager_streaming.rs` | 990 | `crate::table::table_manager_streaming` | `crate::table::table_manager::accessors::table_manager_streaming` | `T` |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | `crates/shamir-engine/src/table/table_manager/transactions/tx_support.rs` | 533 | `crate::table::table_manager_tx_ops` | `crate::table::table_manager::transactions::tx_support` | `T` |
| `crates/shamir-engine/src/table/table_manager_validators.rs` | `crates/shamir-engine/src/table/table_manager/accessors/table_manager_validators.rs` | 473 | `crate::table::table_manager_validators` | `crate::table::table_manager::accessors::table_manager_validators` | `T` |
| `crates/shamir-engine/src/table/tests/and_range_extraction_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/query_ranges/and_range_extraction_tests.rs` | 187 | `crate::table::tests::and_range_extraction_tests` | `crate::table::tests::behavior::query_parity::query_ranges::and_range_extraction_tests` | `T` |
| `crates/shamir-engine/src/table/tests/apply_transforms_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/apply_transforms_tests.rs` | 297 | `crate::table::tests::apply_transforms_tests` | `crate::table::tests::behavior::lifecycle::apply_transforms_tests` | `T` |
| `crates/shamir-engine/src/table/tests/as_of_visibility_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/visibility/as_of_visibility_tests.rs` | 252 | `crate::table::tests::as_of_visibility_tests` | `crate::table::tests::behavior::query_parity::visibility::as_of_visibility_tests` | `T` |
| `crates/shamir-engine/src/table/tests/asof_read_tests.rs` | `crates/shamir-engine/src/table/tests/read/pagination/asof_read_tests.rs` | 508 | `crate::table::tests::asof_read_tests` | `crate::table::tests::read::pagination::asof_read_tests` | `T` |
| `crates/shamir-engine/src/table/tests/batch_hoist_invariants_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/record_view_parity/batch_hoist_invariants_tests.rs` | 400 | `crate::table::tests::batch_hoist_invariants_tests` | `crate::table::tests::behavior::query_parity::record_view_parity::batch_hoist_invariants_tests` | `T` |
| `crates/shamir-engine/src/table/tests/bindings_len_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/bindings_len_tests.rs` | 234 | `crate::table::tests::bindings_len_tests` | `crate::table::tests::behavior::lifecycle::bindings_len_tests` | `T` |
| `crates/shamir-engine/src/table/tests/buffer_config_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/config/buffer_config_tests.rs` | 352 | `crate::table::tests::buffer_config_tests` | `crate::table::tests::table_core::config::buffer_config_tests` | `T` |
| `crates/shamir-engine/src/table/tests/concurrent_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/storage_lifecycle/concurrent_tests.rs` | 392 | `crate::table::tests::concurrent_tests` | `crate::table::tests::table_core::storage_lifecycle::concurrent_tests` | `T` |
| `crates/shamir-engine/src/table/tests/corrupt_record_byte_path_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/record_integrity/corrupt_record_byte_path_tests.rs` | 299 | `crate::table::tests::corrupt_record_byte_path_tests` | `crate::table::tests::table_core::record_integrity::corrupt_record_byte_path_tests` | `T` |
| `crates/shamir-engine/src/table/tests/corrupt_record_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/record_integrity/corrupt_record_tests.rs` | 246 | `crate::table::tests::corrupt_record_tests` | `crate::table::tests::table_core::record_integrity::corrupt_record_tests` | `T` |
| `crates/shamir-engine/src/table/tests/covering_read_tests.rs` | `crates/shamir-engine/src/table/tests/read/filtering/covering_read_tests.rs` | 461 | `crate::table::tests::covering_read_tests` | `crate::table::tests::read::filtering::covering_read_tests` | `T` |
| `crates/shamir-engine/src/table/tests/ddl_op_log_engine_facade_retention_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/registry/ddl_op_log_engine_facade_retention_tests.rs` | 150 | `crate::table::tests::ddl_op_log_engine_facade_retention_tests` | `crate::table::tests::index_lifecycle::registry::ddl_op_log_engine_facade_retention_tests` | `T` |
| `crates/shamir-engine/src/table/tests/degraded_index_count_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/index_state/degraded_index_count_tests.rs` | 688 | `crate::table::tests::degraded_index_count_tests` | `crate::table::tests::index_lifecycle::index_build::index_state::degraded_index_count_tests` | `T` |
| `crates/shamir-engine/src/table/tests/delete_view_parity_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/delete_view_parity_tests.rs` | 433 | `crate::table::tests::delete_view_parity_tests` | `crate::table::tests::write::record_mutation::delete_view_parity_tests` | `T` |
| `crates/shamir-engine/src/table/tests/doctor_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/manager/doctor_tests.rs` | 855 | `crate::table::tests::doctor_tests` | `crate::table::tests::table_core::manager::doctor_tests` | `T` |
| `crates/shamir-engine/src/table/tests/explain_tests.rs` | `crates/shamir-engine/src/table/tests/read/filtering/explain_tests.rs` | 182 | `crate::table::tests::explain_tests` | `crate::table::tests::read::filtering::explain_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f3_cascade_repair_admission_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/admission/f3_cascade_repair_admission_tests.rs` | 93 | `crate::table::tests::f3_cascade_repair_admission_tests` | `crate::table::tests::behavior::admission::f3_cascade_repair_admission_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f50_index_lifecycle_spike_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/online_build/f50_index_lifecycle_spike_tests.rs` | 233 | `crate::table::tests::f50_index_lifecycle_spike_tests` | `crate::table::tests::index_lifecycle::index_build::online_build::f50_index_lifecycle_spike_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f50_step2_index_lifecycle_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/online_build/f50_step2_index_lifecycle_tests.rs` | 459 | `crate::table::tests::f50_step2_index_lifecycle_tests` | `crate::table::tests::index_lifecycle::index_build::online_build::f50_step2_index_lifecycle_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f53a_streaming_topk_tests.rs` | `crates/shamir-engine/src/table/tests/read/streaming/f53a_streaming_topk_tests.rs` | 418 | `crate::table::tests::f53a_streaming_topk_tests` | `crate::table::tests::read::streaming::f53a_streaming_topk_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f53b_asof_seek_tests.rs` | `crates/shamir-engine/src/table/tests/read/pagination/f53b_asof_seek_tests.rs` | 893 | `crate::table::tests::f53b_asof_seek_tests` | `crate::table::tests::read::pagination::f53b_asof_seek_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f53b_cursor_seek_spike_tests.rs` | `crates/shamir-engine/src/table/tests/read/pagination/f53b_cursor_seek_spike_tests.rs` | 806 | `crate::table::tests::f53b_cursor_seek_spike_tests` | `crate::table::tests::read::pagination::f53b_cursor_seek_spike_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f53b_step3_cursor_after_spike_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/query_ranges/f53b_step3_cursor_after_spike_tests.rs` | 557 | `crate::table::tests::f53b_step3_cursor_after_spike_tests` | `crate::table::tests::behavior::query_parity::query_ranges::f53b_step3_cursor_after_spike_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f57_index_create_barrier_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/ddl_barrier/f57_index_create_barrier_tests.rs` | 439 | `crate::table::tests::f57_index_create_barrier_tests` | `crate::table::tests::index_lifecycle::ddl_barrier::f57_index_create_barrier_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f69_write_barrier_single_atomic_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/ddl_barrier/f69_write_barrier_single_atomic_tests.rs` | 335 | `crate::table::tests::f69_write_barrier_single_atomic_tests` | `crate::table::tests::index_lifecycle::ddl_barrier::f69_write_barrier_single_atomic_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f70_lock_order_inversion_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/version_parity/f70_lock_order_inversion_tests.rs` | 283 | `crate::table::tests::f70_lock_order_inversion_tests` | `crate::table::tests::behavior::query_parity::version_parity::f70_lock_order_inversion_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f71_asof_epoch_init_tests.rs` | `crates/shamir-engine/src/table/tests/read/pagination/f71_asof_epoch_init_tests.rs` | 615 | `crate::table::tests::f71_asof_epoch_init_tests` | `crate::table::tests::read::pagination::f71_asof_epoch_init_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f72_planner_invisibility_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/f72_planner_invisibility_tests.rs` | 303 | `crate::table::tests::f72_planner_invisibility_tests` | `crate::table::tests::read::vector::f72_planner_invisibility_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f76_drop_visibility_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/visibility/f76_drop_visibility_tests.rs` | 369 | `crate::table::tests::f76_drop_visibility_tests` | `crate::table::tests::behavior::query_parity::visibility::f76_drop_visibility_tests` | `T` |
| `crates/shamir-engine/src/table/tests/f95_ddl_admission_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/ddl_barrier/f95_ddl_admission_tests.rs` | 497 | `crate::table::tests::f95_ddl_admission_tests` | `crate::table::tests::index_lifecycle::ddl_barrier::f95_ddl_admission_tests` | `T` |
| `crates/shamir-engine/src/table/tests/filter_stream_tests.rs` | `crates/shamir-engine/src/table/tests/read/filtering/filter_stream_tests.rs` | 758 | `crate::table::tests::filter_stream_tests` | `crate::table::tests::read::filtering::filter_stream_tests` | `T` |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann_tests.rs` | 230 | `crate::table::tests::filtered_ann_tests` | `crate::table::tests::read::vector::filtered_ann_tests` | `T` |
| `crates/shamir-engine/src/table/tests/filtered_vector_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_vector_tests.rs` | 88 | `crate::table::tests::filtered_vector_tests` | `crate::table::tests::read::vector::filtered_vector_tests` | `T` |
| `crates/shamir-engine/src/table/tests/fts_tokenizer_dsl_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/schema/fts_tokenizer_dsl_tests.rs` | 178 | `crate::table::tests::fts_tokenizer_dsl_tests` | `crate::table::tests::behavior::schema::fts_tokenizer_dsl_tests` | `T` |
| `crates/shamir-engine/src/table/tests/has_any_index_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/index_state/has_any_index_tests.rs` | 157 | `crate::table::tests::has_any_index_tests` | `crate::table::tests::index_lifecycle::index_build::index_state::has_any_index_tests` | `T` |
| `crates/shamir-engine/src/table/tests/history_read_tests.rs` | `crates/shamir-engine/src/table/tests/read/query/history_read_tests.rs` | 516 | `crate::table::tests::history_read_tests` | `crate::table::tests::read::query::history_read_tests` | `T` |
| `crates/shamir-engine/src/table/tests/in_flight_create_guard_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/instance/in_flight_create_guard_tests.rs` | 79 | `crate::table::tests::in_flight_create_guard_tests` | `crate::table::tests::behavior::lifecycle::instance::in_flight_create_guard_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index2_create_barrier_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/ddl_barrier/index2_create_barrier_tests.rs` | 738 | `crate::table::tests::index2_create_barrier_tests` | `crate::table::tests::index_lifecycle::ddl_barrier::index2_create_barrier_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index2_empty_result_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index2/index2_empty_result_tests.rs` | 234 | `crate::table::tests::index2_empty_result_tests` | `crate::table::tests::index_lifecycle::index2::index2_empty_result_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index2_lifecycle_state_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index2/index2_lifecycle_state_tests.rs` | 421 | `crate::table::tests::index2_lifecycle_state_tests` | `crate::table::tests::index_lifecycle::index2::index2_lifecycle_state_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index2_migration_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/registry/index2_migration_tests.rs` | 287 | `crate::table::tests::index2_migration_tests` | `crate::table::tests::index_lifecycle::registry::index2_migration_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index2_persistence_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index2/index2_persistence_tests.rs` | 379 | `crate::table::tests::index2_persistence_tests` | `crate::table::tests::index_lifecycle::index2::index2_persistence_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index2_read_semantics_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index2/index2_read_semantics_tests.rs` | 423 | `crate::table::tests::index2_read_semantics_tests` | `crate::table::tests::index_lifecycle::index2::index2_read_semantics_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index_build_explicit_pin_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/creation/index_build_explicit_pin_tests.rs` | 235 | `crate::table::tests::index_build_explicit_pin_tests` | `crate::table::tests::index_lifecycle::index_build::creation::index_build_explicit_pin_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index_create_persist_atomicity_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/creation/index_create_persist_atomicity_tests.rs` | 361 | `crate::table::tests::index_create_persist_atomicity_tests` | `crate::table::tests::index_lifecycle::index_build::creation::index_create_persist_atomicity_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index_rename_concurrency_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/index_rename_concurrency_tests.rs` | 302 | `crate::table::tests::index_rename_concurrency_tests` | `crate::table::tests::index_lifecycle::recovery::rename::index_rename_concurrency_tests` | `T` |
| `crates/shamir-engine/src/table/tests/index_scan_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/base_index/index_scan_tests.rs` | 700 | `crate::table::tests::index_scan_tests` | `crate::table::tests::index_lifecycle::base_index::index_scan_tests` | `T` |
| `crates/shamir-engine/src/table/tests/insert_tx_tests.rs` | `crates/shamir-engine/src/table/tests/write/transaction/insert_tx_tests.rs` | 515 | `crate::table::tests::insert_tx_tests` | `crate::table::tests::write::transaction::insert_tx_tests` | `T` |
| `crates/shamir-engine/src/table/tests/interner_manager_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/manager/interner_manager_tests.rs` | 400 | `crate::table::tests::interner_manager_tests` | `crate::table::tests::table_core::manager::interner_manager_tests` | `T` |
| `crates/shamir-engine/src/table/tests/keyset_seek_tests.rs` | `crates/shamir-engine/src/table/tests/read/pagination/keyset_seek_tests.rs` | 766 | `crate::table::tests::keyset_seek_tests` | `crate::table::tests::read::pagination::keyset_seek_tests` | `T` |
| `crates/shamir-engine/src/table/tests/migration_freeze_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs` | 308 | `crate::table::tests::migration_freeze_tests` | `crate::table::tests::index_lifecycle::registry::migration_freeze_tests` | `T` |
| `crates/shamir-engine/src/table/tests/mod.rs` | `crates/shamir-engine/src/table/tests/mod.rs` | 116 | `crate::table::tests` | `crate::table::tests` | `T` |
| `crates/shamir-engine/src/table/tests/multi_vector_index_guard_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/index_state/multi_vector_index_guard_tests.rs` | 143 | `crate::table::tests::multi_vector_index_guard_tests` | `crate::table::tests::index_lifecycle::index_build::index_state::multi_vector_index_guard_tests` | `T` |
| `crates/shamir-engine/src/table/tests/non_tx_ack_wait_tests.rs` | `crates/shamir-engine/src/table/tests/write/transaction/non_tx_ack_wait_tests.rs` | 110 | `crate::table::tests::non_tx_ack_wait_tests` | `crate::table::tests::write::transaction::non_tx_ack_wait_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p02_base_index_rederive_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/base_index/p02_base_index_rederive_tests.rs` | 763 | `crate::table::tests::p02_base_index_rederive_tests` | `crate::table::tests::index_lifecycle::base_index::p02_base_index_rederive_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p02c_retain_filter_key_collision_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/creation/p02c_retain_filter_key_collision_tests.rs` | 152 | `crate::table::tests::p02c_retain_filter_key_collision_tests` | `crate::table::tests::index_lifecycle::index_build::creation::p02c_retain_filter_key_collision_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p03b_index2_drop_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/drop_durability/p03b_index2_drop_durability_tests.rs` | 522 | `crate::table::tests::p03b_index2_drop_durability_tests` | `crate::table::tests::index_lifecycle::recovery::drop_durability::p03b_index2_drop_durability_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1008_instance_provenance_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/instance/p1008_instance_provenance_tests.rs` | 850 | `crate::table::tests::p1008_instance_provenance_tests` | `crate::table::tests::behavior::lifecycle::instance::p1008_instance_provenance_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1011_reader_drain_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/writer_drain/p1011_reader_drain_tests.rs` | 417 | `crate::table::tests::p1011_reader_drain_tests` | `crate::table::tests::index_lifecycle::writer_drain::p1011_reader_drain_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1037_sorted_reader_drain_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/writer_drain/p1037_sorted_reader_drain_tests.rs` | 417 | `crate::table::tests::p1037_sorted_reader_drain_tests` | `crate::table::tests::index_lifecycle::writer_drain::p1037_sorted_reader_drain_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1038_index2_reader_drain_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/writer_drain/p1038_index2_reader_drain_tests.rs` | 427 | `crate::table::tests::p1038_index2_reader_drain_tests` | `crate::table::tests::index_lifecycle::writer_drain::p1038_index2_reader_drain_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1048_hash_drop_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/drop_durability/p1048_hash_drop_durability_tests.rs` | 307 | `crate::table::tests::p1048_hash_drop_durability_tests` | `crate::table::tests::index_lifecycle::recovery::drop_durability::p1048_hash_drop_durability_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1048_index2_drop_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/drop_durability/p1048_index2_drop_durability_tests.rs` | 236 | `crate::table::tests::p1048_index2_drop_durability_tests` | `crate::table::tests::index_lifecycle::recovery::drop_durability::p1048_index2_drop_durability_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1051_op_id_uniqueness_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/instance/p1051_op_id_uniqueness_tests.rs` | 159 | `crate::table::tests::p1051_op_id_uniqueness_tests` | `crate::table::tests::behavior::lifecycle::instance::p1051_op_id_uniqueness_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1057_snapshot_guard_tests.rs` | `crates/shamir-engine/src/table/tests/read/pagination/p1057_snapshot_guard_tests.rs` | 364 | `crate::table::tests::p1057_snapshot_guard_tests` | `crate::table::tests::read::pagination::p1057_snapshot_guard_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1059_online_create_index_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/online_build/p1059_online_create_index_tests.rs` | 427 | `crate::table::tests::p1059_online_create_index_tests` | `crate::table::tests::index_lifecycle::index_build::online_build::p1059_online_create_index_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1060_online_index_crash_recovery_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/recovery/p1060_online_index_crash_recovery_tests.rs` | 566 | `crate::table::tests::p1060_online_index_crash_recovery_tests` | `crate::table::tests::index_lifecycle::recovery::recovery::p1060_online_index_crash_recovery_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1061_pipeline_property_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/pipeline/p1061_pipeline_property_tests.rs` | 721 | `crate::table::tests::p1061_pipeline_property_tests` | `crate::table::tests::behavior::lifecycle::pipeline::p1061_pipeline_property_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1063_multi_fts_index_stats_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/online_build/p1063_multi_fts_index_stats_tests.rs` | 559 | `crate::table::tests::p1063_multi_fts_index_stats_tests` | `crate::table::tests::index_lifecycle::index_build::online_build::p1063_multi_fts_index_stats_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1066_index2_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/p1066_index2_rename_durability_tests.rs` | 503 | `crate::table::tests::p1066_index2_rename_durability_tests` | `crate::table::tests::index_lifecycle::recovery::rename::p1066_index2_rename_durability_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1067_ddl_op_id_recovery_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/recovery/p1067_ddl_op_id_recovery_tests.rs` | 243 | `crate::table::tests::p1067_ddl_op_id_recovery_tests` | `crate::table::tests::index_lifecycle::recovery::recovery::p1067_ddl_op_id_recovery_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1087_phase_b_a_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/pipeline/p1087_phase_b_a_tests.rs` | 232 | `crate::table::tests::p1087_phase_b_a_tests` | `crate::table::tests::behavior::lifecycle::pipeline::p1087_phase_b_a_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1088_phase_c_d_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/pipeline/p1088_phase_c_d_tests.rs` | 490 | `crate::table::tests::p1088_phase_c_d_tests` | `crate::table::tests::behavior::lifecycle::pipeline::p1088_phase_c_d_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p1098_gen_read_order_tests.rs` | `crates/shamir-engine/src/table/tests/read/query/p1098_gen_read_order_tests.rs` | 432 | `crate::table::tests::p1098_gen_read_order_tests` | `crate::table::tests::read::query::p1098_gen_read_order_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p967_ddl_structured_error_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/registry/p967_ddl_structured_error_tests.rs` | 275 | `crate::table::tests::p967_ddl_structured_error_tests` | `crate::table::tests::index_lifecycle::registry::p967_ddl_structured_error_tests` | `T` |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/p997_hash_rename_durability_tests.rs` | 159 | `crate::table::tests::p997_hash_rename_durability_tests` | `crate::table::tests::index_lifecycle::recovery::rename::p997_hash_rename_durability_tests` | `T` |
| `crates/shamir-engine/src/table/tests/persistence_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/storage_lifecycle/persistence_tests.rs` | 357 | `crate::table::tests::persistence_tests` | `crate::table::tests::table_core::storage_lifecycle::persistence_tests` | `T` |
| `crates/shamir-engine/src/table/tests/r0a_registry_watermark_admission_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/ddl_barrier/r0a_registry_watermark_admission_tests.rs` | 473 | `crate::table::tests::r0a_registry_watermark_admission_tests` | `crate::table::tests::index_lifecycle::ddl_barrier::r0a_registry_watermark_admission_tests` | `T` |
| `crates/shamir-engine/src/table/tests/r0c_registry_atomicity_and_namespace_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/registry/r0c_registry_atomicity_and_namespace_tests.rs` | 471 | `crate::table::tests::r0c_registry_atomicity_and_namespace_tests` | `crate::table::tests::index_lifecycle::registry::r0c_registry_atomicity_and_namespace_tests` | `T` |
| `crates/shamir-engine/src/table/tests/r0d_fail_closed_recovery_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/recovery/r0d_fail_closed_recovery_tests.rs` | 432 | `crate::table::tests::r0d_fail_closed_recovery_tests` | `crate::table::tests::index_lifecycle::recovery::recovery::r0d_fail_closed_recovery_tests` | `T` |
| `crates/shamir-engine/src/table/tests/read_one_tx_tests.rs` | `crates/shamir-engine/src/table/tests/read/streaming/read_one_tx_tests.rs` | 239 | `crate::table::tests::read_one_tx_tests` | `crate::table::tests::read::streaming::read_one_tx_tests` | `T` |
| `crates/shamir-engine/src/table/tests/record_counter_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/manager/record_counter_tests.rs` | 451 | `crate::table::tests::record_counter_tests` | `crate::table::tests::table_core::manager::record_counter_tests` | `T` |
| `crates/shamir-engine/src/table/tests/recordview_cutover_parity_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/record_integrity/recordview_cutover_parity_tests.rs` | 603 | `crate::table::tests::recordview_cutover_parity_tests` | `crate::table::tests::table_core::record_integrity::recordview_cutover_parity_tests` | `T` |
| `crates/shamir-engine/src/table/tests/s3_bytes_path_parity_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/record_view_parity/s3_bytes_path_parity_tests.rs` | 356 | `crate::table::tests::s3_bytes_path_parity_tests` | `crate::table::tests::behavior::query_parity::record_view_parity::s3_bytes_path_parity_tests` | `T` |
| `crates/shamir-engine/src/table/tests/s9b_rebuild_on_open_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/pipeline/s9b_rebuild_on_open_tests.rs` | 201 | `crate::table::tests::s9b_rebuild_on_open_tests` | `crate::table::tests::behavior::lifecycle::pipeline::s9b_rebuild_on_open_tests` | `T` |
| `crates/shamir-engine/src/table/tests/s_read_server_tests.rs` | `crates/shamir-engine/src/table/tests/read/query/s_read_server_tests.rs` | 386 | `crate::table::tests::s_read_server_tests` | `crate::table::tests::read::query::s_read_server_tests` | `T` |
| `crates/shamir-engine/src/table/tests/s_write_server_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/s_write_server_tests.rs` | 610 | `crate::table::tests::s_write_server_tests` | `crate::table::tests::write::record_mutation::s_write_server_tests` | `T` |
| `crates/shamir-engine/src/table/tests/schema_activation_barrier_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/ddl_barrier/schema_activation_barrier_tests.rs` | 334 | `crate::table::tests::schema_activation_barrier_tests` | `crate::table::tests::index_lifecycle::ddl_barrier::schema_activation_barrier_tests` | `T` |
| `crates/shamir-engine/src/table/tests/seam_read_tests.rs` | `crates/shamir-engine/src/table/tests/read/streaming/seam_read_tests.rs` | 85 | `crate::table::tests::seam_read_tests` | `crate::table::tests::read::streaming::seam_read_tests` | `T` |
| `crates/shamir-engine/src/table/tests/select_aggregate_context_tests.rs` | `crates/shamir-engine/src/table/tests/read/filtering/select_aggregate_context_tests.rs` | 291 | `crate::table::tests::select_aggregate_context_tests` | `crate::table::tests::read::filtering::select_aggregate_context_tests` | `T` |
| `crates/shamir-engine/src/table/tests/select_expression_reject_tests.rs` | `crates/shamir-engine/src/table/tests/read/filtering/select_expression_reject_tests.rs` | 178 | `crate::table::tests::select_expression_reject_tests` | `crate::table::tests::read::filtering::select_expression_reject_tests` | `T` |
| `crates/shamir-engine/src/table/tests/select_remaining_paths_tests.rs` | `crates/shamir-engine/src/table/tests/read/filtering/select_remaining_paths_tests.rs` | 388 | `crate::table::tests::select_remaining_paths_tests` | `crate::table::tests::read::filtering::select_remaining_paths_tests` | `T` |
| `crates/shamir-engine/src/table/tests/set_byte_merge_parity_tests.rs` | `crates/shamir-engine/src/table/tests/write/upsert/set_byte_merge_parity_tests.rs` | 791 | `crate::table::tests::set_byte_merge_parity_tests` | `crate::table::tests::write::upsert::set_byte_merge_parity_tests` | `T` |
| `crates/shamir-engine/src/table/tests/shortcut_qv_golden_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/visibility/shortcut_qv_golden_tests.rs` | 148 | `crate::table::tests::shortcut_qv_golden_tests` | `crate::table::tests::behavior::query_parity::visibility::shortcut_qv_golden_tests` | `T` |
| `crates/shamir-engine/src/table/tests/snapshot_vector_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/snapshot_vector_tests.rs` | 488 | `crate::table::tests::snapshot_vector_tests` | `crate::table::tests::read::vector::snapshot_vector_tests` | `T` |
| `crates/shamir-engine/src/table/tests/stale_index_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/index_state/stale_index_tests.rs` | 357 | `crate::table::tests::stale_index_tests` | `crate::table::tests::index_lifecycle::index_build::index_state::stale_index_tests` | `T` |
| `crates/shamir-engine/src/table/tests/stream_tx_tests.rs` | `crates/shamir-engine/src/table/tests/read/streaming/stream_tx_tests.rs` | 303 | `crate::table::tests::stream_tx_tests` | `crate::table::tests::read::streaming::stream_tx_tests` | `T` |
| `crates/shamir-engine/src/table/tests/stream_utils.rs` | `crates/shamir-engine/src/table/tests/read/streaming/stream_utils.rs` | 71 | `crate::table::tests::stream_utils` | `crate::table::tests::read::streaming::stream_utils` | `T` |
| `crates/shamir-engine/src/table/tests/table_config_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/config/table_config_tests.rs` | 24 | `crate::table::tests::table_config_tests` | `crate::table::tests::table_core::config::table_config_tests` | `T` |
| `crates/shamir-engine/src/table/tests/table_manager_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/manager/table_manager_tests.rs` | 346 | `crate::table::tests::table_manager_tests` | `crate::table::tests::table_core::manager::table_manager_tests` | `T` |
| `crates/shamir-engine/src/table/tests/table_tests.rs` | `crates/shamir-engine/src/table/tests/table_core/storage_lifecycle/table_tests.rs` | 484 | `crate::table::tests::table_tests` | `crate::table::tests::table_core::storage_lifecycle::table_tests` | `T` |
| `crates/shamir-engine/src/table/tests/test_helpers.rs` | `crates/shamir-engine/src/table/tests/behavior/schema/test_helpers.rs` | 40 | `crate::table::tests::test_helpers` | `crate::table::tests::behavior::schema::test_helpers` | `T` |
| `crates/shamir-engine/src/table/tests/tx_read_shortcut_tests.rs` | `crates/shamir-engine/src/table/tests/read/query/tx_read_shortcut_tests.rs` | 572 | `crate::table::tests::tx_read_shortcut_tests` | `crate::table::tests::read::query::tx_read_shortcut_tests` | `T` |
| `crates/shamir-engine/src/table/tests/tx_scan_overlay_tests.rs` | `crates/shamir-engine/src/table/tests/write/transaction/tx_scan_overlay_tests.rs` | 421 | `crate::table::tests::tx_scan_overlay_tests` | `crate::table::tests::write::transaction::tx_scan_overlay_tests` | `T` |
| `crates/shamir-engine/src/table/tests/update_byte_merge_parity_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/update_byte_merge_parity_tests.rs` | 491 | `crate::table::tests::update_byte_merge_parity_tests` | `crate::table::tests::write::record_mutation::update_byte_merge_parity_tests` | `T` |
| `crates/shamir-engine/src/table/tests/update_validator_gate_tests.rs` | `crates/shamir-engine/src/table/tests/write/transaction/update_validator_gate_tests.rs` | 297 | `crate::table::tests::update_validator_gate_tests` | `crate::table::tests::write::transaction::update_validator_gate_tests` | `T` |
| `crates/shamir-engine/src/table/tests/validator_binding_serialization_tests.rs` | `crates/shamir-engine/src/table/tests/write/transaction/validator_binding_serialization_tests.rs` | 226 | `crate::table::tests::validator_binding_serialization_tests` | `crate::table::tests::write::transaction::validator_binding_serialization_tests` | `T` |
| `crates/shamir-engine/src/table/tests/verify_panic_safety_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/lifecycle/safety/verify_panic_safety_tests.rs` | 92 | `crate::table::tests::verify_panic_safety_tests` | `crate::table::tests::behavior::lifecycle::safety::verify_panic_safety_tests` | `T` |
| `crates/shamir-engine/src/table/tests/version_cas_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/version_parity/version_cas_tests.rs` | 667 | `crate::table::tests::version_cas_tests` | `crate::table::tests::behavior::query_parity::version_parity::version_cas_tests` | `T` |
| `crates/shamir-engine/src/table/tests/with_version_order_by_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/query_parity/version_parity/with_version_order_by_tests.rs` | 473 | `crate::table::tests::with_version_order_by_tests` | `crate::table::tests::behavior::query_parity::version_parity::with_version_order_by_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/write_exec_tests.rs` | 189 | `crate::table::tests::write_exec_tests` | `crate::table::tests::write::record_mutation::write_exec_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_path_preread_fail_closed_tests.rs` | `crates/shamir-engine/src/table/tests/read/query/write_path_preread_fail_closed_tests.rs` | 135 | `crate::table::tests::write_path_preread_fail_closed_tests` | `crate::table::tests::read::query::write_path_preread_fail_closed_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_receipt/as_of_unpublished_gate_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/admission/as_of_unpublished_gate_tests.rs` | 216 | `crate::table::tests::write_receipt::as_of_unpublished_gate_tests` | `crate::table::tests::behavior::admission::as_of_unpublished_gate_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_receipt/crud_tests.rs` | `crates/shamir-engine/src/table/tests/behavior/admission/crud_tests.rs` | 464 | `crate::table::tests::write_receipt::crud_tests` | `crate::table::tests::behavior::admission::crud_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_receipt/mod.rs` | `crates/shamir-engine/src/table/tests/behavior/admission/mod.rs` | 7 | `crate::table::tests::write_receipt` | `crate::table::tests::behavior::admission` | `T` |
| `crates/shamir-engine/src/table/tests/write_receipt/write_domain_admission_matrix_tests.rs` | `crates/shamir-engine/src/table/tests/admission/domain/write_domain_admission_matrix_tests.rs` | 430 | `crate::table::tests::write_receipt::write_domain_admission_matrix_tests` | `crate::table::tests::admission::domain::write_domain_admission_matrix_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_receipt/write_domain_barrier_tests.rs` | `crates/shamir-engine/src/table/tests/admission/barriers/write_domain_barrier_tests.rs` | 41 | `crate::table::tests::write_receipt::write_domain_barrier_tests` | `crate::table::tests::admission::barriers::write_domain_barrier_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_receipt/write_domain_counter_tests.rs` | `crates/shamir-engine/src/table/tests/admission/domain/write_domain_counter_tests.rs` | 123 | `crate::table::tests::write_receipt::write_domain_counter_tests` | `crate::table::tests::admission::domain::write_domain_counter_tests` | `T` |
| `crates/shamir-engine/src/table/tests/write_receipt/write_domain_shared.rs` | `crates/shamir-engine/src/table/tests/admission/domain/write_domain_shared.rs` | 47 | `crate::table::tests::write_receipt::write_domain_shared` | `crate::table::tests::admission::domain::write_domain_shared` | `T` |
| `crates/shamir-engine/src/table/tests/writer_drain_barrier_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/writer_drain/writer_drain_barrier_tests.rs` | 62 | `crate::table::tests::writer_drain_barrier_tests` | `crate::table::tests::index_lifecycle::writer_drain::writer_drain_barrier_tests` | `T` |
| `crates/shamir-engine/src/table/tx_scan_overlay.rs` | `crates/shamir-engine/src/table/support/test_seams/tx_scan_overlay.rs` | 278 | `crate::table::tx_scan_overlay` | `crate::table::support::test_seams::tx_scan_overlay` | `T` |
| `crates/shamir-engine/src/table/write_exec.rs` | `crates/shamir-engine/src/table/write/insert.rs` | 468 | `crate::table::write_exec` | `crate::table::write::insert` | `T` |
| `crates/shamir-engine/src/table/write_helpers.rs` | `crates/shamir-engine/src/table/write/write_helpers.rs` | 508 | `crate::table::write_helpers` | `crate::table::write::write_helpers` | `T` |
| `crates/shamir-engine/src/table/write_receipt/admission/admitted_ops.rs` | `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/admitted_ops.rs` | 290 | `crate::table::write_receipt::admission::admitted_ops` | `crate::table::table_manager::admission::domain::barrier::admitted_ops` | `T` |
| `crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs` | `crates/shamir-engine/src/table/table_manager/admission/views/read_only_views.rs` | 456 | `crate::table::write_receipt::admission::read_only_views` | `crate::table::table_manager::admission::views::read_only_views` | `T` |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_barrier.rs` | `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs` | 540 | `crate::table::write_receipt::admission::write_domain_barrier` | `crate::table::table_manager::admission::domain::barrier::write_domain_barrier` | `T` |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_index_admitted.rs` | `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_admitted.rs` | 477 | `crate::table::write_receipt::admission::write_domain_index_admitted` | `crate::table::table_manager::admission::domain::index_admission::write_domain_index_admitted` | `T` |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_index_core.rs` | `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_core.rs` | 481 | `crate::table::write_receipt::admission::write_domain_index_core` | `crate::table::table_manager::admission::domain::index_admission::write_domain_index_core` | `T` |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_online_index.rs` | `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_online_index.rs` | 370 | `crate::table::write_receipt::admission::write_domain_online_index` | `crate::table::table_manager::admission::domain::index_admission::write_domain_online_index` | `T` |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_rename_admitted.rs` | `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_rename_admitted.rs` | 827 | `crate::table::write_receipt::admission::write_domain_rename_admitted` | `crate::table::table_manager::admission::domain::barrier::write_domain_rename_admitted` | `T` |
| `crates/shamir-engine/src/table/write_receipt/crud.rs` | `crates/shamir-engine/src/table/table_manager/admission/crud.rs` | 140 | `crate::table::write_receipt::crud` | `crate::table::table_manager::admission::crud` | `T` |
| `crates/shamir-engine/src/table/write_receipt/mod.rs` | `crates/shamir-engine/src/table/table_manager/admission/mod.rs` | 4 | `crate::table::write_receipt` | `crate::table::table_manager::admission` | `T` |
| `crates/shamir-engine/src/table/write_receipt/outcome.rs` | `crates/shamir-engine/src/table/table_manager/admission/outcome.rs` | 32 | `crate::table::write_receipt::outcome` | `crate::table::table_manager::admission::outcome` | `T` |
| `crates/shamir-engine/src/table/writer_drain_barrier.rs` | `crates/shamir-engine/src/table/support/guards/writer_drain_barrier.rs` | 590 | `crate::table::writer_drain_barrier` | `crate::table::support::guards::writer_drain_barrier` | `T` |

## Semantic splits (DECIDED; line spans are observed source ranges)

Ranges are 1-based inclusive physical line locations in original committed input; imports/header are charged to first output; spans are enclosing physical coordinates; extraction cuts at actual item/doc/attribute boundaries, assigning each source item exactly once. `expected_lines` is a projected output budget; new outputs charge shared prelude/imports separately as specified below. Actual emitted counts must be recounted. Each output must stay <=1000 lines.

| Source | Output | Lines | Range | Topic | Stage |
|---|---|---:|---:|---|---|
| `crates/shamir-engine/src/table/read_exec.rs` | `crates/shamir-engine/src/table/read/execution/dispatch.rs` | <= 853 | 1–853 | header/imports, version helpers, select predicates, read entry methods, and read_impl dispatch through its closing item at line 852 | T |
| `crates/shamir-engine/src/table/read_exec.rs` | `crates/shamir-engine/src/table/read/dispatch/scan.rs` | <= 266 | 854–1119 | read_scan and snapshot keyset | T |
| `crates/shamir-engine/src/table/read_exec.rs` | `crates/shamir-engine/src/table/read/dispatch/collect.rs` | <= 384 | 1120–1503 | collecting scan and postprocessing | T |
| `crates/shamir-engine/src/table/read_exec.rs` | `crates/shamir-engine/src/table/read/dispatch/paged.rs` | <= 415 | 1504–1918 | counting and streaming | T |
| `crates/shamir-engine/src/table/read_exec.rs` | `crates/shamir-engine/src/table/read/dispatch/vector.rs` | <= 780 | 1919–2698 | snapshot/filtered vector and candidate phases | T |
| `crates/shamir-engine/src/table/read_exec.rs` | `crates/shamir-engine/src/table/read/dispatch/explain.rs` | <= 394 | 2699–3092 | explain and projection | T |
| `crates/shamir-engine/src/table/table_manager.rs` | `crates/shamir-engine/src/table/table_manager.rs` | <= 703 | 1–703 | header/imports, path-child declarations, TableManager fields, support structs/constants, Clone impl, and constructor impl through line 703; fields remain defining owner | T |
| `crates/shamir-engine/src/table/table_manager.rs` | `crates/shamir-engine/src/table/table_manager/accessors/accessors.rs` | <= 364 | 704–1067 | inherent TableManager methods kept as child modules of the defining module; fields remain private to owner, and all required table-super consumers remain in scope | T |
| `crates/shamir-engine/src/table/table_manager.rs` | `crates/shamir-engine/src/table/table_manager/barriers/barriers.rs` | <= 350 | 1068–1417 | barrier/drain inherent methods are declaring-module descendants; preserve private flag/lock access and cancellation/drop ordering | T |
| `crates/shamir-engine/src/table/table_manager.rs` | `crates/shamir-engine/src/table/table_manager/barriers/guard.rs` | <= 87 | 1418–1504 | barrier guard | T |
| `crates/shamir-engine/src/table/table_manager.rs` | `crates/shamir-engine/src/table/table_manager/accessors/tokenizer.rs` | <= 32 | 1505–1536 | tokenizer DSL | T |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | `crates/shamir-engine/src/table/table_manager/transactions/tx_support.rs` | <= 533 | 1–533 | header/imports, StagedMutation, #1098 seam, released-unique cache, and planner/staging helper impl through line 502; complete documentation ends at 533 | T |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | `crates/shamir-engine/src/table/table_manager/transactions/tx_insert.rs` | <= 549 | 534–1082 | transactional inserts | T |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | `crates/shamir-engine/src/table/table_manager/transactions/tx_update.rs` | <= 360 | 1083–1442 | transactional updates | T |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | `crates/shamir-engine/src/table/table_manager/transactions/tx_delete.rs` | <= 110 | 1443–1552 | transactional delete and set | T |
| `crates/shamir-engine/src/table/write_exec.rs` | `crates/shamir-engine/src/table/write/insert.rs` | <= 468 | 1–468 | header/imports, version precondition helper, and full execute_insert_tx method through line 459 | T |
| `crates/shamir-engine/src/table/write_exec.rs` | `crates/shamir-engine/src/table/write/update/update.rs` | <= 398 | 469–866 | update execution | T |
| `crates/shamir-engine/src/table/write_exec.rs` | `crates/shamir-engine/src/table/write/delete/delete.rs` | <= 250 | 867–1116 | delete execution | T |
| `crates/shamir-engine/src/table/write_exec.rs` | `crates/shamir-engine/src/table/write/set/set.rs` | <= 336 | 1117–1452 | upsert execution | T |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/fixtures/filtered_ann_tests.rs` | <= 230 | 1–230 | fixtures/bare ANN | T |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/selectivity.rs` | <= 145 | 231–375 | selectivity scenario group | T |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/residual_filter.rs` | <= 227 | 376–602 | residual filter scenario group | T |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/prefilter_cofilter.rs` | <= 241 | 603–843 | prefilter cofilter scenario group | T |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/staged_vector.rs` | <= 108 | 844–951 | staged vector scenario group | T |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/postfilter_retry.rs` | <= 81 | 952–1032 | postfilter retry scenario group | T |
| `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/staged_update_dedup.rs` | <= 75 | 1033–1107 | staged update dedup scenario group | T |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/fixtures/p997_hash_rename_durability_tests.rs` | <= 159 | 1–159 | fixtures/regular recovery | T |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/cases/regular_recovery.rs` | <= 188 | 160–347 | regular recovery scenario group | T |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/cases/unique_recovery.rs` | <= 248 | 348–595 | unique recovery scenario group | T |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/cases/live_rename_crash.rs` | <= 246 | 596–841 | live rename crash scenario group | T |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/cases/rename_parity.rs` | <= 119 | 842–960 | rename parity scenario group | T |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/cases/multi_entry_recovery.rs` | <= 137 | 961–1097 | multi entry recovery scenario group | T |
| `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/cases/op_id_compat.rs` | <= 58 | 1098–1155 | op id compat scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/fixtures/write_exec_tests.rs` | <= 189 | 1–189 | fixtures/basic mutations | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/returning/mutation_returning.rs` | <= 224 | 190–413 | mutation returning scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/cases/set_upsert.rs` | <= 174 | 414–587 | set upsert scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/cases/persistence.rs` | <= 48 | 588–635 | persistence scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/cases/computed_success.rs` | <= 90 | 636–725 | computed success scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/cases/computed_fail_closed.rs` | <= 84 | 726–809 | computed fail closed scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/cases/non_tx_parity.rs` | <= 53 | 810–862 | non tx parity scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/cases/validation.rs` | <= 87 | 863–949 | validation scenario group | T |
| `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | `crates/shamir-engine/src/table/tests/write/record_mutation/execution/misc/remaining.rs` | <= 60 | 950–1009 | remaining scenario group | T |

### Split review notes

* `read_exec.rs`: split at method boundaries: `read_impl`, `read_scan`, snapshot keyset, collecting, counting, streaming, vector phases, explain and projections. Header/imports belong only to dispatcher; shared helpers stay with explicit owner. Preserve planner fallback and corrupt-row behavior. IMPORTANT: moving inherent impl methods into sibling modules makes their private methods module-private and inaccessible to other impl siblings. Keep calls co-located/descendant through explicit `pub(super)` only after enumerating callers, or retain the relevant private helpers together; this is a manual review obligation, not compile-proven.
* `table_manager.rs`: type definition, complete field set, clone and constructors stay together in the defining module. Methods that touch fields remain descendant implementation modules declared by that owner; no sibling moves and no visibility widening. The `pub(super)` methods in neighboring table modules must retain the table-parent access scope or have all callers explicitly accounted for.
* `table_manager_tx_ops.rs`: shared staging helpers and insert/update/delete/set methods are complete item groups; preserve generation capture before index snapshots and incremental unique-cache semantics.
* `write_exec.rs`: split by complete insert/update/delete/set method groups. Review validator-before-stage, per-row staging, transaction cancellation and result assembly.
* Oversized tests use test/function boundaries observed in each source; helper fixtures stay with dependent cases. Preserve test names and all `cfg`, ignore and loom attributes exactly. Line spans are enclosing ranges and require item-boundary verification during extraction.

## Privacy, mounts and restricted visibility (OBSERVED)

`table_manager.rs:25–37` has seven real `#[path]` attributes for admission adapter files; lines 20–24 are explanatory comments, not attributes. These modules are logical children of `TableManager`'s defining module. Replace mounts with ordinary child declarations rooted at `table_manager.rs` and place adapters under its descendant directory. `TableManager` field sites are at `table_manager.rs:116–429`; do not place field-touching adapters as siblings or widen field visibility.

Restricted visibility sites observed: `table_manager_tx_ops.rs:15–18` (`StagedMutation` and members), `table_manager_crud.rs:14,102,131,149,185,296,475,567`, `table_manager_index_mgmt.rs:64,178,937`, `table_manager_streaming.rs:352,509,561`, and `read_exec.rs:79,107,259,269,1121,1505,1647,2880,2965,3051`. `pub(in crate::table)` at index management lines 64 and 937 is table-scoped. Audit meaning after moves and retain all required consumers under the same owner scope; no blanket visibility changes.

## Path mounts and module meaning

The seven attributes at `table_manager.rs:25–37` mount `write_receipt/admission/*.rs` as children of the defining module, enabling access to its private/`pub(super)` fields. They are true attributes, distinct from three comment-string hits in the broad inventory. Proposal: ordinary descendant modules under `table_manager/admission/{domain,views}/`, preserving actual logical ownership and access scope. Table remains under `src/table`; packing unrelated engine roots is out of scope.

## Placement and limitations

The complete tree remains below `crates/shamir-engine/src/table/`. Semantic production groups are `read/`, `write/`, `table_manager/`, `model/`, `index/`, `write_receipt/`, and `support/`; semantic test groups are `tests/read/`, `tests/write/`, `tests/table_core/`, `tests/index_lifecycle/`, `tests/admission/`, and `tests/behavior/`. Each folder has a manifest row below or is represented by a relocated tracked `mod.rs`. Manifests contain declarations/re-exports only.

JSON line counts and split sizes are DECIDED projections, not compile-proven. Before execution, count the entire projected tree, including every manifest and secondary split output; ensure each folder has at most seven direct entries and each file at most 1,000 lines. No Rust/source/config/baseline changes were made; no cargo, build, or test commands were run.

### New manifests and declarations (DECIDED)

| New manifest path | Lines | Declaration purpose |
|---|---:|---|

## New manifests (DECIDED; exact projected declarations below)

| New manifest path | Lines |
|---|---:|
| `crates/shamir-engine/src/table/read/dispatch/mod.rs` | 35 |
| `crates/shamir-engine/src/table/read/execution/mod.rs` | 35 |
| `crates/shamir-engine/src/table/read/planning/mod.rs` | 35 |
| `crates/shamir-engine/src/table/read/vector/mod.rs` | 35 |
| `crates/shamir-engine/src/table/support/guards/mod.rs` | 35 |
| `crates/shamir-engine/src/table/support/model/mod.rs` | 35 |
| `crates/shamir-engine/src/table/support/storage/mod.rs` | 35 |
| `crates/shamir-engine/src/table/support/test_seams/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/accessors/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/admission/views/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/barriers/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/diagnostics/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/operations/mod.rs` | 35 |
| `crates/shamir-engine/src/table/table_manager/transactions/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/admission/barriers/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/admission/domain/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/lifecycle/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/lifecycle/instance/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/lifecycle/pipeline/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/lifecycle/safety/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/query_parity/query_ranges/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/query_parity/record_view_parity/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/query_parity/version_parity/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/query_parity/visibility/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/behavior/schema/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/base_index/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/ddl_barrier/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/index2/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/creation/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/index_state/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/index_build/online_build/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/drop_durability/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/recovery/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/cases/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/recovery/rename/hash_rename/fixtures/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/registry/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/index_lifecycle/writer_drain/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/read/filtering/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/read/pagination/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/read/query/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/read/streaming/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/read/vector/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/fixtures/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/table_core/config/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/table_core/manager/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/table_core/record_integrity/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/table_core/storage_lifecycle/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/write/record_mutation/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/write/record_mutation/execution/cases/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/write/record_mutation/execution/fixtures/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/write/record_mutation/execution/misc/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/write/record_mutation/execution/returning/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/write/transaction/mod.rs` | 35 |
| `crates/shamir-engine/src/table/tests/write/upsert/mod.rs` | 35 |
| `crates/shamir-engine/src/table/write/mod.rs` | 35 |
| `crates/shamir-engine/src/table/write/delete/mod.rs` | 35 |
| `crates/shamir-engine/src/table/write/set/mod.rs` | 35 |
| `crates/shamir-engine/src/table/write/update/mod.rs` | 35 |

## Exact projected module declarations (DECIDED from the JSON tree)

Immediate child declarations are derived from every projected Rust path, including primary and secondary split outputs. Existing tracked manifests retain their existing declarations (listed in source; re-export details below). Use `#[cfg(test)]` on test-only child declarations. Directory manifests contain declarations only; never elevate visibility to public by default.

| Manifest owner | Immediate declaration | Projected Rust child path |
|---|---|---|

### Existing facade declarations/re-exports (OBSERVED)

| Source | Line | Exact declaration |
|---|---:|---|
| `crates/shamir-engine/src/table/mod.rs` | 1 | `pub mod buffer_config;` |
| `crates/shamir-engine/src/table/mod.rs` | 3 | `pub use shamir_index::base_index::ddl_op_log;` |
| `crates/shamir-engine/src/table/mod.rs` | 5 | `pub mod degraded_index_count;` |
| `crates/shamir-engine/src/table/mod.rs` | 6 | `pub mod doctor;` |
| `crates/shamir-engine/src/table/mod.rs` | 8 | `pub mod in_flight_create_guard;` |
| `crates/shamir-engine/src/table/mod.rs` | 10 | `pub mod index2_backfill_hook;` |
| `crates/shamir-engine/src/table/mod.rs` | 11 | `pub mod interner_manager;` |
| `crates/shamir-engine/src/table/mod.rs` | 13 | `pub mod persistable;` |
| `crates/shamir-engine/src/table/mod.rs` | 19 | `pub mod record_counter;` |
| `crates/shamir-engine/src/table/mod.rs` | 20 | `pub mod record_cow;` |
| `crates/shamir-engine/src/table/mod.rs` | 22 | `pub mod table;` |
| `crates/shamir-engine/src/table/mod.rs` | 23 | `pub mod table_config;` |
| `crates/shamir-engine/src/table/mod.rs` | 24 | `pub mod table_manager;` |
| `crates/shamir-engine/src/table/mod.rs` | 36 | `pub(crate) use table_manager_tx_ops::refresh_released_unique_cache;` |
| `crates/shamir-engine/src/table/mod.rs` | 40 | `pub(crate) mod write_helpers;` |
| `crates/shamir-engine/src/table/mod.rs` | 41 | `pub mod write_receipt;` |
| `crates/shamir-engine/src/table/mod.rs` | 42 | `pub use write_receipt::WriteOutcome;` |
| `crates/shamir-engine/src/table/mod.rs` | 43 | `pub mod writer_drain_barrier;` |
| `crates/shamir-engine/src/table/mod.rs` | 46 | `pub mod tests;` |
| `crates/shamir-engine/src/table/mod.rs` | 48 | `pub use interner_manager::InternerManager;` |
| `crates/shamir-engine/src/table/mod.rs` | 49 | `pub use migration_freeze_guard::MigrationFreezeGuard;` |
| `crates/shamir-engine/src/table/mod.rs` | 50 | `pub use persistable::{PersistRegistry, Persistable};` |
| `crates/shamir-engine/src/table/mod.rs` | 51 | `pub use record_counter::RecordCounter;` |
| `crates/shamir-engine/src/table/mod.rs` | 52 | `pub use table_config::TableConfig;` |
| `crates/shamir-engine/src/table/mod.rs` | 53 | `pub use table_manager::{` |
| `crates/shamir-engine/src/table/mod.rs` | 59 | `pub use table::Table;` |
| `crates/shamir-engine/src/table/mod.rs` | 65 | `pub(crate) use table_manager_streaming::{ReadOneTxBytesFailHook, TEST_READ_ONE_TX_BYTES_FAILURE};` |
| `crates/shamir-engine/src/table/mod.rs` | 72 | `pub(crate) use read_asof_seek::{SeekLoopPreIterHook, TEST_SEEK_LOOP_PRE_ITER_HOOK};` |

### Actual path mounts (OBSERVED)

Only attributes are listed (not comments containing the text `#[path]`). Proposed replacement is ordinary declaration beneath `table_manager.rs`; preserve the logical child scope.

| Owner source | Line | Literal mount target | Proposed ordinary child scope |
|---|---:|---|---|
| `crates/shamir-engine/src/table/table_manager.rs` | 25 | `write_receipt/admission/admitted_ops.rs` | `crate::table::table_manager::admission::admitted_ops` |
| `crates/shamir-engine/src/table/table_manager.rs` | 27 | `write_receipt/admission/read_only_views.rs` | `crate::table::table_manager::admission::read_only_views` |
| `crates/shamir-engine/src/table/table_manager.rs` | 29 | `write_receipt/admission/write_domain_barrier.rs` | `crate::table::table_manager::admission::write_domain_barrier` |
| `crates/shamir-engine/src/table/table_manager.rs` | 31 | `write_receipt/admission/write_domain_index_admitted.rs` | `crate::table::table_manager::admission::write_domain_index_admitted` |
| `crates/shamir-engine/src/table/table_manager.rs` | 33 | `write_receipt/admission/write_domain_index_core.rs` | `crate::table::table_manager::admission::write_domain_index_core` |
| `crates/shamir-engine/src/table/table_manager.rs` | 35 | `write_receipt/admission/write_domain_online_index.rs` | `crate::table::table_manager::admission::write_domain_online_index` |
| `crates/shamir-engine/src/table/table_manager.rs` | 37 | `write_receipt/admission/write_domain_rename_admitted.rs` | `crate::table::table_manager::admission::write_domain_rename_admitted` |

### Restricted visibility sites (OBSERVED source text)

Every non-comment declaration line containing `pub(super)` or `pub(in ...)` in the table subtree is listed. Meaning after move is DECIDED only as an obligation: preserve the original ancestor scope; enumerate all callers before moving. `pub(crate)` declarations are not restricted sites in this inventory.

| Source | Line | Observed declaration | Relocation obligation |
|---|---:|---|---|
| `crates/shamir-engine/src/table/candidate_set.rs` | 8 | `pub(super) struct CandidateSet {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/candidate_set.rs` | 28 | `pub(super) fn index2_shortcut_eligible(query: &ReadQuery) -> bool {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/migration_freeze_guard.rs` | 15 | `pub(super) fn try_set(flags: WriteBarrierFlags) -> Option<Self> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_asof_seek.rs` | 180 | `pub(super) async fn read_as_of_keyset_seek(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 79 | `pub(super) fn collect_versions(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 107 | `pub(super) fn versions_from_matched(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 259 | `pub(super) async fn read_for_tx(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 269 | `pub(super) async fn read_for_tx_with_encoding(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 1121 | `pub(super) async fn read_collecting(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 1505 | `pub(super) async fn read_counting(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 1647 | `pub(super) async fn read_streaming(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 2880 | `pub(super) fn try_project_page_only(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 2965 | `pub(super) fn try_project_page_only_bytes(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_exec.rs` | 3051 | `pub(super) fn apply_select_value_bytes(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_index_scan.rs` | 35 | `pub(super) async fn read_sorted_index_scan(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_index_scan.rs` | 400 | `pub(super) async fn read_order_limit_fast(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_index_scan.rs` | 498 | `pub(super) async fn read_keyset_seek(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_index_scan.rs` | 637 | `pub(super) async fn read_index_scan(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_planner.rs` | 35 | `pub(super) async fn try_plan_index2(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_planner.rs` | 588 | `pub(super) fn encode_filter_value_for_sort(value: &FilterValue) -> Option<Vec<u8>> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_planner.rs` | 605 | `pub(super) fn encode_query_value_for_sort(value: &QueryValue) -> Option<Vec<u8>> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_temporal.rs` | 49 | `pub(super) async fn read_as_of(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/read_temporal.rs` | 443 | `pub(super) async fn read_history(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table.rs` | 232 | `pub(super) fn list_stream_filtered<'a>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table.rs` | 269 | `pub(super) fn decode_raw_batch(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table.rs` | 303 | `pub(super) fn decode_raw_batch_filtered(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 116 | `pub(super) name: String,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 117 | `pub(super) table: Arc<Table>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 122 | `pub(super) info_store: Arc<dyn Store>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 123 | `pub(super) interner: InternerManager,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 124 | `pub(super) counter: Arc<RecordCounter>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 127 | `pub(super) persist_registry: PersistRegistry,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 128 | `pub(super) index_manager: IndexManager,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 129 | `pub(super) sorted_indexes: SortedIndexManager,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 134 | `pub(super) write_counter: Arc<std::sync::atomic::AtomicU64>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 137 | `pub(super) verify_running: Arc<std::sync::atomic::AtomicBool>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 142 | `pub(super) unique_write_lock: Arc<tokio::sync::Mutex<()>>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 206 | `pub(super) write_barrier_flags: crate::index::write_barrier_flags::WriteBarrierFlags,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 236 | `pub(super) ddl_admission: Arc<tokio::sync::Mutex<()>>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 244 | `pub(super) write_domain: Arc<RepoWriteDomain>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 256 | `pub(super) writer_drain: super::writer_drain_barrier::WriterDrainBarrier,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 272 | `pub(super) in_flight_creates: super::in_flight_create_guard::InFlightCreateSet,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 273 | `pub(super) index2_registry: Arc<crate::index2::IndexRegistry>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 278 | `pub(super) filtered_ann_thresholds: Option<(usize, f64)>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 279 | `pub(super) mvcc_store: Option<Arc<shamir_tx::MvccStore>>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 284 | `pub(super) validator_bindings: Arc<arc_swap::ArcSwap<Vec<crate::validator::ValidatorBinding>>>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 286 | `pub(super) bindings_lock: Arc<tokio::sync::Mutex<()>>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 291 | `pub(super) validator_registry: Option<Arc<crate::validator::ValidatorRegistry>>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 298 | `pub(super) changefeed: Option<NonTxChangefeed>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 303 | `pub(super) scalar_resolver:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 313 | `pub(super) create_index2_backfill_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 324 | `pub(super) create_sorted_index_backfill_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 334 | `pub(super) drop_index2_pause_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 345 | `pub(super) drop_index2_post_sweep_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 355 | `pub(super) rename_index2_early_pause_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 367 | `pub(super) rename_index2_mid_pause_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 380 | `pub(super) index2_registered_before_ready_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 390 | `pub(super) online_index_backfill_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 397 | `pub(super) phase_b_pause_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 404 | `pub(super) phase_c_pause_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 411 | `pub(super) phase_d_pause_hook:` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 422 | `pub(super) verify_panic_hook: Arc<std::sync::atomic::AtomicBool>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 429 | `pub(super) verify_call_count: Arc<std::sync::atomic::AtomicUsize>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 441 | `pub(super) struct NonTxChangefeed {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 442 | `pub(super) gate: Arc<shamir_tx::RepoTxGate>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager.rs` | 462 | `pub(super) const AUTO_VERIFY_EVERY_N_WRITES: u64 = 1024;` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 14 | `pub(super) fn ensure_repo_readable(&self) -> DbResult<()> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 102 | `pub(super) async fn index2_on_insert(&self, rid: &RecordId, rec: &InnerValue) -> DbResult<()> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 131 | `pub(super) async fn index2_on_update(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 149 | `pub(super) async fn index2_on_delete(&self, rid: &RecordId, rec: &InnerValue) -> DbResult<()> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 185 | `pub(super) async fn insert_unacknowledged(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 296 | `pub(super) async fn insert_many_unacknowledged(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 475 | `pub(super) async fn delete_unacknowledged(&self, id: RecordId) -> DbResult<(bool, u64)> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_crud.rs` | 567 | `pub(super) async fn set_unacknowledged(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` | 64 | `pub(in crate::table) async fn reject_create_while_drop_in_flight(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` | 178 | `pub(super) async fn backfill_index2_backend(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` | 937 | `pub(in crate::table) async fn build_index_definition(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_streaming.rs` | 352 | `pub(super) fn list_stream_tx_after_predicate<'a>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_streaming.rs` | 509 | `pub(super) fn record_scan_reads<'a, S>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_streaming.rs` | 561 | `pub(super) async fn collect_all_current_records(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 15 | `pub(super) struct StagedMutation {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 16 | `pub(super) data_op: KvOp,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 17 | `pub(super) index_ops: Vec<shamir_tx::IndexWriteOp>,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 18 | `pub(super) counter_delta: i64,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 154 | `pub(super) fn mark_staged_ids_captured(&self, tx: &mut shamir_tx::TxContext) {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 161 | `pub(super) async fn stage_mutation(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 191 | `pub(super) async fn plan_insert_ops(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 226 | `pub(super) async fn stamp_index2_provenance(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 266 | `pub(super) async fn stage_vectors(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 291 | `pub(super) async fn stage_vector_delete<R>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 316 | `pub(super) async fn stage_vector_deletes_on_update<Old, New>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 340 | `pub(super) async fn plan_update_ops(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 363 | `pub(super) async fn plan_update_ops_ref<R>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 392 | `pub(super) async fn plan_delete_ops<R>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 420 | `pub(super) async fn plan_base_index_insert_ops(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 436 | `pub(super) async fn plan_base_index_update_ops(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 459 | `pub(super) async fn plan_base_index_update_ops_ref<R>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 485 | `pub(super) async fn plan_base_index_delete_ops<R>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/table_manager_validators.rs` | 253 | `pub(super) async fn run_validators_view(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/tx_scan_overlay.rs` | 38 | `pub(super) enum StagedRowOverlay {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/tx_scan_overlay.rs` | 55 | `pub(super) fn overlay_rows_for_tx(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/tx_scan_overlay.rs` | 87 | `pub(super) fn merge_stream_with_tx_overlay<'a>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/tx_scan_overlay.rs` | 104 | `pub(super) fn merge_filtered_stream_with_tx_overlay<'a, F>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/tx_scan_overlay.rs` | 139 | `pub(super) fn staged_only_matches<F>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 30 | `pub(super) fn validator_failure_to_db_error(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 71 | `pub(super) fn is_computed_field(v: &QueryValue) -> bool {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 93 | `pub(super) fn resolve_computed_record<'a>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 161 | `pub(super) fn apply_defaults(rec: &mut QueryValue, defaults: &[(Vec<String>, QueryValue)]) {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 291 | `pub(super) fn eval_write_value(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 326 | `pub(super) fn qv_nav<'a>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 342 | `pub(super) fn make_layered_interner<'a>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 355 | `pub(super) fn intern_via_layered<'a>(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 380 | `pub(super) async fn lookup_records_via_index(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_helpers.rs` | 451 | `pub(super) async fn lookup_existing_for_set(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs` | 83 | `pub(super) manager: &'a TableManager,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs` | 328 | `pub(super) manager: &'a TableManager,` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_barrier.rs` | 39 | `pub(in crate::table) async fn admit(&self) -> DbResult<RepoWritePermit> {` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_barrier.rs` | 60 | `pub(super) async fn begin_write_barrier_admitted_impl(` | Preserve original table-parent/owner descendant reach; no blanket widening |
| `crates/shamir-engine/src/table/write_receipt/admission/write_domain_index_core.rs` | 180 | `pub(in crate::table) async fn create_unique_index_body(` | Preserve original table-parent/owner descendant reach; no blanket widening |

### Test-hook consumer search (OBSERVED)

Search covers all engine Rust source for the three `pub(crate)` transaction test-hook names. The name `refresh_released_unique_cache` has the cross-module test consumer below; hook-type and static consumers are confined to table tests. No consumers were found in `shamir-db`, `shamir-server`, or engine benches for these three hooks.

| Consumer source | Line | Reference |
|---|---:|---|
| `crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check_tests.rs` | 378 | `crate::table::refresh_released_unique_cache(&mut tx, token);` |

### Stage T acceptance and manual review

Dependencies: facade/hooks stable (A1) and frozen shared table API. Stage T may run parallel to Q only when shared ownership is serialized. Scope: all `src/table/**`; tests move with their implementation owners. Acceptance: complete item/test conservation in CI, ordinary modules, no table `#[path]` mounts or visibility widening, no layout regressions. Review guard/lifetime scope, cancellation, drop order, resource ownership, write-admission and durability ordering; loom-model correspondence requires manual review. None of these claims is compile-proven here.
