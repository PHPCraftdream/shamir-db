# Engine sweep manifest — query, validator, benches and integration tests (Q)

Status: DECIDED relocation/split proposal; no source edits or compile/discovery were performed. All tables below are emitted by this script from git ls-files, actual tracked source bytes and the committed layout baseline.

## Scoped baseline inventory

| Kind | Path | Count | Stage |
|---|---|---:|---|
| dir | `crates/shamir-engine/benches` | 39 | Q |
| dir | `crates/shamir-engine/src/query` | 8 | Q |
| dir | `crates/shamir-engine/src/query/batch` | 19 | Q |
| dir | `crates/shamir-engine/src/query/batch/tests` | 33 | Q |
| dir | `crates/shamir-engine/src/query/batch/tests/executor_tests` | 12 | Q |
| dir | `crates/shamir-engine/src/query/filter` | 16 | Q |
| dir | `crates/shamir-engine/src/query/filter/tests` | 8 | Q |
| dir | `crates/shamir-engine/src/query/filter/tests/eval_tests` | 19 | Q |
| dir | `crates/shamir-engine/src/query/read` | 8 | Q |
| dir | `crates/shamir-engine/src/query/read/tests` | 11 | Q |
| dir | `crates/shamir-engine/src/validator` | 18 | Q |
| dir | `crates/shamir-engine/src/validator/schema` | 11 | Q |
| dir | `crates/shamir-engine/src/validator/tests` | 13 | Q |
| file | `crates/shamir-engine/benches/tx_concurrent.rs` | 1013 | Q |
| file | `crates/shamir-engine/src/query/batch/fk_actions.rs` | 1438 | Q |
| file | `crates/shamir-engine/src/query/batch/fk_on_update.rs` | 1353 | Q |
| file | `crates/shamir-engine/src/query/batch/query_runner.rs` | 2006 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | 1688 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | 2481 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs` | 1147 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs` | 1341 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | 2151 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs` | 1509 | Q |
| file | `crates/shamir-engine/src/query/filter/filter_node.rs` | 1099 | Q |
| file | `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs` | 1318 | Q |
| file | `crates/shamir-engine/src/query/read/aggregate.rs` | 1779 | Q |
| file | `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | 1222 | Q |
| file | `crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs` | 1354 | Q |
| file | `crates/shamir-engine/tests/crash_recovery.rs` | 1651 | Q |

OBSERVED: assigned scope contains 234 tracked paths; baseline violation rows plus scoped root census rows are listed below. The checker excludes `README.md` from source line enforcement, although the global inventory's oversized-source list includes the 1,351-line batch README. Its observed line count is not a file violation.

## Exhaustive source-to-destination mapping

Every tracked path in the assigned prefixes occurs exactly once. `Lines` is the projected primary file size when it is split; README rows retain their measured count but are outside the source-extension line limit in check_layout.py. Bench and integration test files remain Cargo targets, not regular module children.

| Source | Destination | Lines | Logical old | Logical new | Stage |
|---|---|---:|---|---|---|
| `crates/shamir-engine/benches/backend_matrix.rs` | `crates/shamir-engine/benches/backend/backend_matrix.rs` | 136 | `Cargo bench target backend_matrix` | `Cargo bench target backend_matrix (explicit [[bench]] path crates/shamir-engine/benches/backend/backend_matrix.rs)` | Q |
| `crates/shamir-engine/benches/batch_stage_parallelism.rs` | `crates/shamir-engine/benches/query_writes/batch_ops/batch_stage_parallelism.rs` | 202 | `Cargo bench target batch_stage_parallelism` | `Cargo bench target batch_stage_parallelism (explicit [[bench]] path crates/shamir-engine/benches/query_writes/batch_ops/batch_stage_parallelism.rs)` | Q |
| `crates/shamir-engine/benches/cond_expr_eval.rs` | `crates/shamir-engine/benches/query_reads/filtering/cond_expr_eval.rs` | 290 | `Cargo bench target cond_expr_eval` | `Cargo bench target cond_expr_eval (explicit [[bench]] path crates/shamir-engine/benches/query_reads/filtering/cond_expr_eval.rs)` | Q |
| `crates/shamir-engine/benches/cursor_pages_depth.rs` | `crates/shamir-engine/benches/query_reads/ordering/cursor_pages_depth.rs` | 151 | `Cargo bench target cursor_pages_depth` | `Cargo bench target cursor_pages_depth (explicit [[bench]] path crates/shamir-engine/benches/query_reads/ordering/cursor_pages_depth.rs)` | Q |
| `crates/shamir-engine/benches/distinct.rs` | `crates/shamir-engine/benches/query_reads/ordering/distinct.rs` | 93 | `Cargo bench target distinct` | `Cargo bench target distinct (explicit [[bench]] path crates/shamir-engine/benches/query_reads/ordering/distinct.rs)` | Q |
| `crates/shamir-engine/benches/drain_cost_vs_depth.rs` | `crates/shamir-engine/benches/durability/drain_cost_vs_depth.rs` | 139 | `Cargo bench target drain_cost_vs_depth` | `Cargo bench target drain_cost_vs_depth (explicit [[bench]] path crates/shamir-engine/benches/durability/drain_cost_vs_depth.rs)` | Q |
| `crates/shamir-engine/benches/drain_throughput.rs` | `crates/shamir-engine/benches/durability/drain_throughput.rs` | 106 | `Cargo bench target drain_throughput` | `Cargo bench target drain_throughput (explicit [[bench]] path crates/shamir-engine/benches/durability/drain_throughput.rs)` | Q |
| `crates/shamir-engine/benches/durable_concurrent_commit.rs` | `crates/shamir-engine/benches/durability/durable_concurrent_commit.rs` | 164 | `Cargo bench target durable_concurrent_commit` | `Cargo bench target durable_concurrent_commit (explicit [[bench]] path crates/shamir-engine/benches/durability/durable_concurrent_commit.rs)` | Q |
| `crates/shamir-engine/benches/f78_writer_latency.rs` | `crates/shamir-engine/benches/durability/f78_writer_latency.rs` | 230 | `Cargo bench target f78_writer_latency` | `Cargo bench target f78_writer_latency (explicit [[bench]] path crates/shamir-engine/benches/durability/f78_writer_latency.rs)` | Q |
| `crates/shamir-engine/benches/f80_writer_drain_overhead.rs` | `crates/shamir-engine/benches/durability/f80_writer_drain_overhead.rs` | 174 | `Cargo bench target f80_writer_drain_overhead` | `Cargo bench target f80_writer_drain_overhead (explicit [[bench]] path crates/shamir-engine/benches/durability/f80_writer_drain_overhead.rs)` | Q |
| `crates/shamir-engine/benches/filter_eval.rs` | `crates/shamir-engine/benches/query_reads/filtering/filter_eval.rs` | 389 | `Cargo bench target filter_eval` | `Cargo bench target filter_eval (explicit [[bench]] path crates/shamir-engine/benches/query_reads/filtering/filter_eval.rs)` | Q |
| `crates/shamir-engine/benches/filtered_vector_search.rs` | `crates/shamir-engine/benches/vector_search/filtered_vector_search.rs` | 179 | `Cargo bench target filtered_vector_search` | `Cargo bench target filtered_vector_search (explicit [[bench]] path crates/shamir-engine/benches/vector_search/filtered_vector_search.rs)` | Q |
| `crates/shamir-engine/benches/fk_cascade_index.rs` | `crates/shamir-engine/benches/query_writes/constraints/fk_cascade_index.rs` | 349 | `Cargo bench target fk_cascade_index` | `Cargo bench target fk_cascade_index (explicit [[bench]] path crates/shamir-engine/benches/query_writes/constraints/fk_cascade_index.rs)` | Q |
| `crates/shamir-engine/benches/for_each_eval.rs` | `crates/shamir-engine/benches/query_writes/batch_ops/for_each_eval.rs` | 523 | `Cargo bench target for_each_eval` | `Cargo bench target for_each_eval (explicit [[bench]] path crates/shamir-engine/benches/query_writes/batch_ops/for_each_eval.rs)` | Q |
| `crates/shamir-engine/benches/fts_indexed.rs` | `crates/shamir-engine/benches/backend/fts_indexed.rs` | 303 | `Cargo bench target fts_indexed` | `Cargo bench target fts_indexed (explicit [[bench]] path crates/shamir-engine/benches/backend/fts_indexed.rs)` | Q |
| `crates/shamir-engine/benches/group_by_keys.rs` | `crates/shamir-engine/benches/query_reads/selection/group_by_keys.rs` | 214 | `Cargo bench target group_by_keys` | `Cargo bench target group_by_keys (explicit [[bench]] path crates/shamir-engine/benches/query_reads/selection/group_by_keys.rs)` | Q |
| `crates/shamir-engine/benches/interner_cold_growth.rs` | `crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/interner_cold_growth.rs` | 260 | `Cargo bench target interner_cold_growth` | `Cargo bench target interner_cold_growth (explicit [[bench]] path crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/interner_cold_growth.rs)` | Q |
| `crates/shamir-engine/benches/interner_concurrent.rs` | `crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/interner_concurrent.rs` | 131 | `Cargo bench target interner_concurrent` | `Cargo bench target interner_concurrent (explicit [[bench]] path crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/interner_concurrent.rs)` | Q |
| `crates/shamir-engine/benches/keyset_seek.rs` | `crates/shamir-engine/benches/backend/keyset_seek.rs` | 121 | `Cargo bench target keyset_seek` | `Cargo bench target keyset_seek (explicit [[bench]] path crates/shamir-engine/benches/backend/keyset_seek.rs)` | Q |
| `crates/shamir-engine/benches/membuffer_concurrent.rs` | `crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/membuffer_concurrent.rs` | 130 | `Cargo bench target membuffer_concurrent` | `Cargo bench target membuffer_concurrent (explicit [[bench]] path crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/membuffer_concurrent.rs)` | Q |
| `crates/shamir-engine/benches/membuffer_pump.rs` | `crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/membuffer_pump.rs` | 127 | `Cargo bench target membuffer_pump` | `Cargo bench target membuffer_pump (explicit [[bench]] path crates/shamir-engine/benches/storage_concurrency/buffers_and_interner/membuffer_pump.rs)` | Q |
| `crates/shamir-engine/benches/order_by_pipeline.rs` | `crates/shamir-engine/benches/query_reads/ordering/order_by_pipeline.rs` | 301 | `Cargo bench target order_by_pipeline` | `Cargo bench target order_by_pipeline (explicit [[bench]] path crates/shamir-engine/benches/query_reads/ordering/order_by_pipeline.rs)` | Q |
| `crates/shamir-engine/benches/p1099_touched_probe.rs` | `crates/shamir-engine/benches/query_writes/constraints/p1099_touched_probe.rs` | 234 | `Cargo bench target p1099_touched_probe` | `Cargo bench target p1099_touched_probe (explicit [[bench]] path crates/shamir-engine/benches/query_writes/constraints/p1099_touched_probe.rs)` | Q |
| `crates/shamir-engine/benches/p1107_stale_value_gate.rs` | `crates/shamir-engine/benches/query_writes/constraints/p1107_stale_value_gate.rs` | 342 | `Cargo bench target p1107_stale_value_gate` | `Cargo bench target p1107_stale_value_gate (explicit [[bench]] path crates/shamir-engine/benches/query_writes/constraints/p1107_stale_value_gate.rs)` | Q |
| `crates/shamir-engine/benches/p1108_released_unique_growth.rs` | `crates/shamir-engine/benches/query_writes/constraints/p1108_released_unique_growth.rs` | 128 | `Cargo bench target p1108_released_unique_growth` | `Cargo bench target p1108_released_unique_growth (explicit [[bench]] path crates/shamir-engine/benches/query_writes/constraints/p1108_released_unique_growth.rs)` | Q |
| `crates/shamir-engine/benches/permission_check.rs` | `crates/shamir-engine/benches/query_writes/batch_ops/permission_check.rs` | 128 | `Cargo bench target permission_check` | `Cargo bench target permission_check (explicit [[bench]] path crates/shamir-engine/benches/query_writes/batch_ops/permission_check.rs)` | Q |
| `crates/shamir-engine/benches/quantization_f32_vs_sq8.rs` | `crates/shamir-engine/benches/vector_search/quantization_f32_vs_sq8.rs` | 306 | `Cargo bench target quantization_f32_vs_sq8` | `Cargo bench target quantization_f32_vs_sq8 (explicit [[bench]] path crates/shamir-engine/benches/vector_search/quantization_f32_vs_sq8.rs)` | Q |
| `crates/shamir-engine/benches/read_path_matrix.rs` | `crates/shamir-engine/benches/query_reads/ordering/read_path_matrix.rs` | 401 | `Cargo bench target read_path_matrix` | `Cargo bench target read_path_matrix (explicit [[bench]] path crates/shamir-engine/benches/query_reads/ordering/read_path_matrix.rs)` | Q |
| `crates/shamir-engine/benches/replication_apply.rs` | `crates/shamir-engine/benches/durability/replication_apply.rs` | 105 | `Cargo bench target replication_apply` | `Cargo bench target replication_apply (explicit [[bench]] path crates/shamir-engine/benches/durability/replication_apply.rs)` | Q |
| `crates/shamir-engine/benches/select_pipeline.rs` | `crates/shamir-engine/benches/query_reads/selection/select_pipeline.rs` | 406 | `Cargo bench target select_pipeline` | `Cargo bench target select_pipeline (explicit [[bench]] path crates/shamir-engine/benches/query_reads/selection/select_pipeline.rs)` | Q |
| `crates/shamir-engine/benches/select_projection.rs` | `crates/shamir-engine/benches/query_reads/selection/select_projection.rs` | 158 | `Cargo bench target select_projection` | `Cargo bench target select_projection (explicit [[bench]] path crates/shamir-engine/benches/query_reads/selection/select_projection.rs)` | Q |
| `crates/shamir-engine/benches/streaming_topk.rs` | `crates/shamir-engine/benches/query_reads/selection/streaming_topk.rs` | 189 | `Cargo bench target streaming_topk` | `Cargo bench target streaming_topk (explicit [[bench]] path crates/shamir-engine/benches/query_reads/selection/streaming_topk.rs)` | Q |
| `crates/shamir-engine/benches/tx_aware_reads.rs` | `crates/shamir-engine/benches/storage_concurrency/transactions/tx_aware_reads.rs` | 220 | `Cargo bench target tx_aware_reads` | `Cargo bench target tx_aware_reads (explicit [[bench]] path crates/shamir-engine/benches/storage_concurrency/transactions/tx_aware_reads.rs)` | Q |
| `crates/shamir-engine/benches/tx_concurrent.rs` | `crates/shamir-engine/benches/storage_concurrency/transactions/tx_concurrent.rs` | 238 | `Cargo bench target tx_concurrent` | `Cargo bench target tx_concurrent (explicit [[bench]] path crates/shamir-engine/benches/storage_concurrency/transactions/tx_concurrent.rs)` | Q |
| `crates/shamir-engine/benches/tx_pipeline.rs` | `crates/shamir-engine/benches/storage_concurrency/transactions/tx_pipeline.rs` | 963 | `Cargo bench target tx_pipeline` | `Cargo bench target tx_pipeline (explicit [[bench]] path crates/shamir-engine/benches/storage_concurrency/transactions/tx_pipeline.rs)` | Q |
| `crates/shamir-engine/benches/vector_bulk_compaction.rs` | `crates/shamir-engine/benches/vector_search/vector_bulk_compaction.rs` | 226 | `Cargo bench target vector_bulk_compaction` | `Cargo bench target vector_bulk_compaction (explicit [[bench]] path crates/shamir-engine/benches/vector_search/vector_bulk_compaction.rs)` | Q |
| `crates/shamir-engine/benches/vector_search.rs` | `crates/shamir-engine/benches/vector_search/vector_search.rs` | 217 | `Cargo bench target vector_search` | `Cargo bench target vector_search (explicit [[bench]] path crates/shamir-engine/benches/vector_search/vector_search.rs)` | Q |
| `crates/shamir-engine/benches/wasm_invoke.rs` | `crates/shamir-engine/benches/backend/wasm_invoke.rs` | 219 | `Cargo bench target wasm_invoke` | `Cargo bench target wasm_invoke (explicit [[bench]] path crates/shamir-engine/benches/backend/wasm_invoke.rs)` | Q |
| `crates/shamir-engine/benches/when_skip_eval.rs` | `crates/shamir-engine/benches/query_reads/filtering/when_skip_eval.rs` | 354 | `Cargo bench target when_skip_eval` | `Cargo bench target when_skip_eval (explicit [[bench]] path crates/shamir-engine/benches/query_reads/filtering/when_skip_eval.rs)` | Q |
| `crates/shamir-engine/src/query/README.md` | `crates/shamir-engine/src/query/admin/docs/README.md` | 432 | `not a Rust item` | `crate::query::admin::docs::README.md` | Q |
| `crates/shamir-engine/src/query/admin/README.md` | `crates/shamir-engine/src/query/admin/README.md` | 103 | `not a Rust item` | `crate::query::admin::README.md` | Q |
| `crates/shamir-engine/src/query/admin/mod.rs` | `crates/shamir-engine/src/query/admin/mod.rs` | 18 | `crate::query::admin` | `crate::query::admin` | Q |
| `crates/shamir-engine/src/query/auth/README.md` | `crates/shamir-engine/src/query/auth/README.md` | 292 | `not a Rust item` | `crate::query::auth::README.md` | Q |
| `crates/shamir-engine/src/query/auth/mod.rs` | `crates/shamir-engine/src/query/auth/mod.rs` | 30 | `crate::query::auth` | `crate::query::auth` | Q |
| `crates/shamir-engine/src/query/auth/session.rs` | `crates/shamir-engine/src/query/auth/session.rs` | 672 | `crate::query::auth::session` | `crate::query::auth::session` | Q |
| `crates/shamir-engine/src/query/auth/tests/mod.rs` | `crates/shamir-engine/src/query/auth/tests/mod.rs` | 1 | `crate::query::auth::tests` | `crate::query::auth::tests` | Q |
| `crates/shamir-engine/src/query/auth/tests/session_tests.rs` | `crates/shamir-engine/src/query/auth/tests/session_tests.rs` | 202 | `crate::query::auth::tests::session_tests` | `crate::query::auth::tests::session_tests` | Q |
| `crates/shamir-engine/src/query/batch/README.md` | `crates/shamir-engine/src/query/batch/api/README.md` | 1351 | `not a Rust item` | `crate::query::batch::api::README.md` | Q |
| `crates/shamir-engine/src/query/batch/authorized.rs` | `crates/shamir-engine/src/query/batch/api/authorized.rs` | 222 | `crate::query::batch::authorized` | `crate::query::batch::api::authorized` | Q |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | `crates/shamir-engine/src/query/batch/api/batch_execute.rs` | 959 | `crate::query::batch::batch_execute` | `crate::query::batch::api::batch_execute` | Q |
| `crates/shamir-engine/src/query/batch/batch_validate.rs` | `crates/shamir-engine/src/query/batch/api/batch_validate.rs` | 208 | `crate::query::batch::batch_validate` | `crate::query::batch::api::batch_validate` | Q |
| `crates/shamir-engine/src/query/batch/exec_scope.rs` | `crates/shamir-engine/src/query/batch/runtime/exec_scope.rs` | 259 | `crate::query::batch::exec_scope` | `crate::query::batch::runtime::exec_scope` | Q |
| `crates/shamir-engine/src/query/batch/execution_deadline.rs` | `crates/shamir-engine/src/query/batch/runtime/execution_deadline.rs` | 316 | `crate::query::batch::execution_deadline` | `crate::query::batch::runtime::execution_deadline` | Q |
| `crates/shamir-engine/src/query/batch/executor_traits.rs` | `crates/shamir-engine/src/query/batch/api/executor_traits.rs` | 65 | `crate::query::batch::executor_traits` | `crate::query::batch::api::executor_traits` | Q |
| `crates/shamir-engine/src/query/batch/fk_actions.rs` | `crates/shamir-engine/src/query/batch/foreign_key/cascade/planning.rs` | 607 | `crate::query::batch::fk_actions` | `crate::query::batch::foreign_key::cascade::planning` | Q |
| `crates/shamir-engine/src/query/batch/fk_on_update.rs` | `crates/shamir-engine/src/query/batch/foreign_key/update/update_plan.rs` | 571 | `crate::query::batch::fk_on_update` | `crate::query::batch::foreign_key::update::update_plan` | Q |
| `crates/shamir-engine/src/query/batch/fk_restrict.rs` | `crates/shamir-engine/src/query/batch/foreign_key/fk_restrict.rs` | 615 | `crate::query::batch::fk_restrict` | `crate::query::batch::foreign_key::fk_restrict` | Q |
| `crates/shamir-engine/src/query/batch/interactive_tx.rs` | `crates/shamir-engine/src/query/batch/api/interactive_tx.rs` | 181 | `crate::query::batch::interactive_tx` | `crate::query::batch::api::interactive_tx` | Q |
| `crates/shamir-engine/src/query/batch/mod.rs` | `crates/shamir-engine/src/query/batch/mod.rs` | 199 | `crate::query::batch` | `crate::query::batch` | Q |
| `crates/shamir-engine/src/query/batch/op_watchdog.rs` | `crates/shamir-engine/src/query/batch/runtime/op_watchdog.rs` | 204 | `crate::query::batch::op_watchdog` | `crate::query::batch::runtime::op_watchdog` | Q |
| `crates/shamir-engine/src/query/batch/param_subst.rs` | `crates/shamir-engine/src/query/batch/query/support/param_subst.rs` | 275 | `crate::query::batch::param_subst` | `crate::query::batch::query::support::param_subst` | Q |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | `crates/shamir-engine/src/query/batch/query/query_runner.rs` | 663 | `crate::query::batch::query_runner` | `crate::query::batch::query::query_runner` | Q |
| `crates/shamir-engine/src/query/batch/query_value_serializer.rs` | `crates/shamir-engine/src/query/batch/query/support/query_value_serializer.rs` | 464 | `crate::query::batch::query_value_serializer` | `crate::query::batch::query::support::query_value_serializer` | Q |
| `crates/shamir-engine/src/query/batch/receipts/accounting.rs` | `crates/shamir-engine/src/query/batch/receipts/accounting.rs` | 71 | `crate::query::batch::receipts::accounting` | `crate::query::batch::receipts::accounting` | Q |
| `crates/shamir-engine/src/query/batch/receipts/commit.rs` | `crates/shamir-engine/src/query/batch/receipts/commit.rs` | 76 | `crate::query::batch::receipts::commit` | `crate::query::batch::receipts::commit` | Q |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 167 | `crate::query::batch::receipts::dependency` | `crate::query::batch::receipts::dependency` | Q |
| `crates/shamir-engine/src/query/batch/receipts/mod.rs` | `crates/shamir-engine/src/query/batch/receipts/mod.rs` | 16 | `crate::query::batch::receipts` | `crate::query::batch::receipts` | Q |
| `crates/shamir-engine/src/query/batch/receipts/policy.rs` | `crates/shamir-engine/src/query/batch/receipts/policy.rs` | 120 | `crate::query::batch::receipts::policy` | `crate::query::batch::receipts::policy` | Q |
| `crates/shamir-engine/src/query/batch/receipts/propagation.rs` | `crates/shamir-engine/src/query/batch/receipts/propagation.rs` | 187 | `crate::query::batch::receipts::propagation` | `crate::query::batch::receipts::propagation` | Q |
| `crates/shamir-engine/src/query/batch/receipts/tests/batch_tests.rs` | `crates/shamir-engine/src/query/batch/receipts/tests/batch_tests.rs` | 398 | `crate::query::batch::receipts::tests::batch_tests` | `crate::query::batch::receipts::tests::batch_tests` | Q |
| `crates/shamir-engine/src/query/batch/receipts/tests/commit_error_code_tests.rs` | `crates/shamir-engine/src/query/batch/receipts/tests/commit_error_code_tests.rs` | 30 | `crate::query::batch::receipts::tests::commit_error_code_tests` | `crate::query::batch::receipts::tests::commit_error_code_tests` | Q |
| `crates/shamir-engine/src/query/batch/receipts/tests/mod.rs` | `crates/shamir-engine/src/query/batch/receipts/tests/mod.rs` | 2 | `crate::query::batch::receipts::tests` | `crate::query::batch::receipts::tests` | Q |
| `crates/shamir-engine/src/query/batch/result_size.rs` | `crates/shamir-engine/src/query/batch/runtime/result_size.rs` | 177 | `crate::query::batch::result_size` | `crate::query::batch::runtime::result_size` | Q |
| `crates/shamir-engine/src/query/batch/tests/authorized_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/authorized_tests.rs` | 235 | `crate::query::batch::tests::authorized_tests` | `crate::query::batch::tests::executor::authorized_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/call_tx_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/call_tx_tests.rs` | 299 | `crate::query::batch::tests::call_tx_tests` | `crate::query::batch::tests::executor::call_tx_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/cross_repo_guard_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/cross_repo_guard_tests.rs` | 165 | `crate::query::batch::tests::cross_repo_guard_tests` | `crate::query::batch::tests::executor::cross_repo_guard_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/deadline_cap_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/control/deadline_cap_tests.rs` | 99 | `crate::query::batch::tests::deadline_cap_tests` | `crate::query::batch::tests::limits::control::deadline_cap_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/dos_gate_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/security/dos_gate_tests.rs` | 902 | `crate::query::batch::tests::dos_gate_tests` | `crate::query::batch::tests::limits::security::dos_gate_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/exec_scope_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/plan_refs/exec_scope_tests.rs` | 108 | `crate::query::batch::tests::exec_scope_tests` | `crate::query::batch::tests::query_semantics::plan_refs::exec_scope_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/basic_ops_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/basic/basic_ops_tests.rs` | 261 | `crate::query::batch::tests::executor_tests::basic_ops_tests` | `crate::query::batch::tests::executor::nested::basic::basic_ops_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/shared/common.rs` | 131 | `crate::query::batch::tests::executor_tests::common` | `crate::query::batch::tests::executor::nested::shared::common` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/error_handling_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/basic/error_handling_tests.rs` | 101 | `crate::query::batch::tests::executor_tests::error_handling_tests` | `crate::query::batch::tests::executor::nested::basic::error_handling_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/interactive_tx_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/transactions/interactive_tx_tests.rs` | 653 | `crate::query::batch::tests::executor_tests::interactive_tx_tests` | `crate::query::batch::tests::executor::nested::transactions::interactive_tx_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/mod.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/shared/mod.rs` | 11 | `crate::query::batch::tests::executor_tests` | `crate::query::batch::tests::executor::nested::shared` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/permissions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/basic/permissions_tests.rs` | 306 | `crate::query::batch::tests::executor_tests::permissions_tests` | `crate::query::batch::tests::executor::nested::basic::permissions_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/query_refs_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/queries/query_refs_tests.rs` | 229 | `crate::query::batch::tests::executor_tests::query_refs_tests` | `crate::query::batch::tests::executor::nested::queries::query_refs_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/query_runner_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/queries/query_runner_tests.rs` | 323 | `crate::query::batch::tests::executor_tests::query_runner_tests` | `crate::query::batch::tests::executor::nested::queries::query_runner_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/ssi_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/transactions/ssi_tests.rs` | 239 | `crate::query::batch::tests::executor_tests::ssi_tests` | `crate::query::batch::tests::executor::nested::transactions::ssi_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/transactions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/transactions/transactions_tests.rs` | 173 | `crate::query::batch::tests::executor_tests::transactions_tests` | `crate::query::batch::tests::executor::nested::transactions::transactions_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/when_skip_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/queries/when_skip_tests.rs` | 808 | `crate::query::batch::tests::executor_tests::when_skip_tests` | `crate::query::batch::tests::executor::nested::queries::when_skip_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/write_value_resolution_tests.rs` | `crates/shamir-engine/src/query/batch/tests/executor/nested/queries/write_value_resolution_tests.rs` | 894 | `crate::query::batch::tests::executor_tests::write_value_resolution_tests` | `crate::query::batch::tests::executor::nested::queries::write_value_resolution_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/filter_guard_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/core/filter_guard_tests.rs` | 389 | `crate::query::batch::tests::filter_guard_tests` | `crate::query::batch::tests::query_semantics::core::filter_guard_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/delete/cascade_tests.rs` | 580 | `crate::query::batch::tests::fk_actions_tests` | `crate::query::batch::tests::referential_actions::delete::cascade_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_discovery_fail_closed_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/core/fk_discovery_fail_closed_tests.rs` | 213 | `crate::query::batch::tests::fk_discovery_fail_closed_tests` | `crate::query::batch::tests::referential_actions::core::fk_discovery_fail_closed_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_fail_closed_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/core/fk_fail_closed_tests.rs` | 320 | `crate::query::batch::tests::fk_fail_closed_tests` | `crate::query::batch::tests::referential_actions::core::fk_fail_closed_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/core/fk_indexed_action_read_error_tests.rs` | 647 | `crate::query::batch::tests::fk_indexed_action_read_error_tests` | `crate::query::batch::tests::referential_actions::core::fk_indexed_action_read_error_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/update/on_update_core_tests.rs` | 742 | `crate::query::batch::tests::fk_on_update_tests` | `crate::query::batch::tests::referential_actions::update::on_update_core_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/integration/implicit_delete_races.rs` | 731 | `crate::query::batch::tests::fk_race_closure_tests` | `crate::query::batch::tests::referential_races::integration::implicit_delete_races` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_restrict_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/core/fk_restrict_tests.rs` | 719 | `crate::query::batch::tests::fk_restrict_tests` | `crate::query::batch::tests::referential_actions::core::fk_restrict_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_reverse_cache_race_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/cache/fk_reverse_cache_race_tests.rs` | 664 | `crate::query::batch::tests::fk_reverse_cache_race_tests` | `crate::query::batch::tests::referential_races::cache::fk_reverse_cache_race_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_reverse_cache_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/cache/fk_reverse_cache_tests.rs` | 467 | `crate::query::batch::tests::fk_reverse_cache_tests` | `crate::query::batch::tests::referential_races::cache::fk_reverse_cache_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/integration/explicit_delete_races.rs` | 559 | `crate::query::batch::tests::fk_ri_barrier_tests` | `crate::query::batch::tests::referential_races::integration::explicit_delete_races` | Q |
| `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations/for_each_core_tests.rs` | 747 | `crate::query::batch::tests::for_each_tests` | `crate::query::batch::tests::query_semantics::nested_batch::iterations::for_each_core_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/index_visibility_parity_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/core/index_visibility_parity_tests.rs` | 125 | `crate::query::batch::tests::index_visibility_parity_tests` | `crate::query::batch::tests::query_semantics::core::index_visibility_parity_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/interactive_deadline_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/control/interactive_deadline_tests.rs` | 250 | `crate::query::batch::tests::interactive_deadline_tests` | `crate::query::batch::tests::limits::control::interactive_deadline_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/limit_enforcement_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/control/limit_enforcement_tests.rs` | 274 | `crate::query::batch::tests::limit_enforcement_tests` | `crate::query::batch::tests::limits::control::limit_enforcement_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/mod.rs` | `crates/shamir-engine/src/query/batch/tests/mod.rs` | 32 | `crate::query::batch::tests` | `crate::query::batch::tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/pagination_bounds_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/core/pagination_bounds_tests.rs` | 60 | `crate::query::batch::tests::pagination_bounds_tests` | `crate::query::batch::tests::query_semantics::core::pagination_bounds_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/planner_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/plan_refs/planner_tests.rs` | 884 | `crate::query::batch::tests::planner_tests` | `crate::query::batch::tests::query_semantics::plan_refs::planner_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/query_value_serializer_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/plan_refs/query_value_serializer_tests.rs` | 476 | `crate::query::batch::tests::query_value_serializer_tests` | `crate::query::batch::tests::query_semantics::plan_refs::query_value_serializer_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/reference_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/plan_refs/reference_tests.rs` | 269 | `crate::query::batch::tests::reference_tests` | `crate::query::batch::tests::query_semantics::plan_refs::reference_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/rowwise_byte_budget_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/rowwise_byte_budget_tests.rs` | 259 | `crate::query::batch::tests::rowwise_byte_budget_tests` | `crate::query::batch::tests::limits::rowwise_byte_budget_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/runtime_byte_budget_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/runtime_byte_budget_tests.rs` | 473 | `crate::query::batch::tests::runtime_byte_budget_tests` | `crate::query::batch::tests::limits::runtime_byte_budget_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/runtime_work_counter_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/runtime_work_counter_tests.rs` | 403 | `crate::query::batch::tests::runtime_work_counter_tests` | `crate::query::batch::tests::limits::runtime_work_counter_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/sub_batch/sub_batch_execution_tests.rs` | 825 | `crate::query::batch::tests::sub_batch_tests` | `crate::query::batch::tests::query_semantics::nested_batch::sub_batch::sub_batch_execution_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/subscribe_guard_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/core/subscribe_guard_tests.rs` | 137 | `crate::query::batch::tests::subscribe_guard_tests` | `crate::query::batch::tests::query_semantics::core::subscribe_guard_tests` | Q |
| `crates/shamir-engine/src/query/batch/tests/watchdog_tests.rs` | `crates/shamir-engine/src/query/batch/tests/limits/control/watchdog_tests.rs` | 640 | `crate::query::batch::tests::watchdog_tests` | `crate::query::batch::tests::limits::control::watchdog_tests` | Q |
| `crates/shamir-engine/src/query/filter/README.md` | `crates/shamir-engine/src/query/filter/README.md` | 116 | `not a Rust item` | `crate::query::filter::README.md` | Q |
| `crates/shamir-engine/src/query/filter/compile.rs` | `crates/shamir-engine/src/query/filter/evaluation/planning/compile.rs` | 322 | `crate::query::filter::compile` | `crate::query::filter::evaluation::planning::compile` | Q |
| `crates/shamir-engine/src/query/filter/cond_cache.rs` | `crates/shamir-engine/src/query/filter/caches/cond_cache.rs` | 232 | `crate::query::filter::cond_cache` | `crate::query::filter::caches::cond_cache` | Q |
| `crates/shamir-engine/src/query/filter/eval.rs` | `crates/shamir-engine/src/query/filter/evaluation/runtime/core/eval.rs` | 14 | `crate::query::filter::eval` | `crate::query::filter::evaluation::runtime::core::eval` | Q |
| `crates/shamir-engine/src/query/filter/eval_bytes.rs` | `crates/shamir-engine/src/query/filter/evaluation/runtime/core/eval_bytes.rs` | 650 | `crate::query::filter::eval_bytes` | `crate::query::filter::evaluation::runtime::core::eval_bytes` | Q |
| `crates/shamir-engine/src/query/filter/eval_context.rs` | `crates/shamir-engine/src/query/filter/evaluation/runtime/core/eval_context.rs` | 237 | `crate::query::filter::eval_context` | `crate::query::filter::evaluation::runtime::core::eval_context` | Q |
| `crates/shamir-engine/src/query/filter/field_path_cache.rs` | `crates/shamir-engine/src/query/filter/caches/field_path_cache.rs` | 177 | `crate::query::filter::field_path_cache` | `crate::query::filter::caches::field_path_cache` | Q |
| `crates/shamir-engine/src/query/filter/filter_node.rs` | `crates/shamir-engine/src/query/filter/evaluation/filter_node/mod.rs` | 785 | `crate::query::filter::filter_node` | `crate::query::filter::evaluation::filter_node` | Q |
| `crates/shamir-engine/src/query/filter/fts.rs` | `crates/shamir-engine/src/query/filter/evaluation/semantics/fts.rs` | 87 | `crate::query::filter::fts` | `crate::query::filter::evaluation::semantics::fts` | Q |
| `crates/shamir-engine/src/query/filter/index_range.rs` | `crates/shamir-engine/src/query/filter/evaluation/planning/index_range.rs` | 229 | `crate::query::filter::index_range` | `crate::query::filter::evaluation::planning::index_range` | Q |
| `crates/shamir-engine/src/query/filter/mod.rs` | `crates/shamir-engine/src/query/filter/mod.rs` | 46 | `crate::query::filter` | `crate::query::filter` | Q |
| `crates/shamir-engine/src/query/filter/numeric_cmp.rs` | `crates/shamir-engine/src/query/filter/evaluation/semantics/numeric_cmp.rs` | 152 | `crate::query::filter::numeric_cmp` | `crate::query::filter::evaluation::semantics::numeric_cmp` | Q |
| `crates/shamir-engine/src/query/filter/pattern_guard.rs` | `crates/shamir-engine/src/query/filter/evaluation/semantics/pattern_guard.rs` | 190 | `crate::query::filter::pattern_guard` | `crate::query::filter::evaluation::semantics::pattern_guard` | Q |
| `crates/shamir-engine/src/query/filter/query_ref_cache.rs` | `crates/shamir-engine/src/query/filter/caches/query_ref_cache.rs` | 227 | `crate::query::filter::query_ref_cache` | `crate::query::filter::caches::query_ref_cache` | Q |
| `crates/shamir-engine/src/query/filter/resolve.rs` | `crates/shamir-engine/src/query/filter/evaluation/semantics/resolve.rs` | 835 | `crate::query::filter::resolve` | `crate::query::filter::evaluation::semantics::resolve` | Q |
| `crates/shamir-engine/src/query/filter/tests/cond_cache_tests.rs` | `crates/shamir-engine/src/query/filter/tests/evaluation/cond_cache_tests.rs` | 597 | `crate::query::filter::tests::cond_cache_tests` | `crate::query::filter::tests::evaluation::cond_cache_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_bytes_tests.rs` | `crates/shamir-engine/src/query/filter/tests/evaluation/eval_bytes_tests.rs` | 450 | `crate::query::filter::tests::eval_bytes_tests` | `crate::query::filter::tests::evaluation::eval_bytes_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/collections/membership_literals_tests.rs` | 661 | `crate::query::filter::tests::eval_tests::collection_tests` | `crate::query::filter::tests::eval_tests::collections::membership_literals_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/comparison_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/comparisons/comparison_tests.rs` | 111 | `crate::query::filter::tests::eval_tests::comparison_tests` | `crate::query::filter::tests::eval_tests::comparisons::comparison_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/computed_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/expressions/computed_tests.rs` | 244 | `crate::query::filter::tests::eval_tests::computed_tests` | `crate::query::filter::tests::eval_tests::expressions::computed_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/cond_expr_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/expressions/cond_expr_tests.rs` | 301 | `crate::query::filter::tests::eval_tests::cond_expr_tests` | `crate::query::filter::tests::eval_tests::expressions::cond_expr_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/cond_gap_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/expressions/cond_gap_tests.rs` | 313 | `crate::query::filter::tests::eval_tests::cond_gap_tests` | `crate::query::filter::tests::eval_tests::expressions::cond_gap_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/dec_cross_type_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/comparisons/dec_cross_type_tests.rs` | 650 | `crate::query::filter::tests::eval_tests::dec_cross_type_tests` | `crate::query::filter::tests::eval_tests::comparisons::dec_cross_type_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/field_ref_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/references/field_ref_tests.rs` | 60 | `crate::query::filter::tests::eval_tests::field_ref_tests` | `crate::query::filter::tests::eval_tests::references::field_ref_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/fts_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/predicates/fts_tests.rs` | 82 | `crate::query::filter::tests::eval_tests::fts_tests` | `crate::query::filter::tests::eval_tests::predicates::fts_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/shared/helpers.rs` | 108 | `crate::query::filter::tests::eval_tests::helpers` | `crate::query::filter::tests::eval_tests::shared::helpers` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/in_query_ref_column_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/references/in_query_ref_column_tests.rs` | 361 | `crate::query::filter::tests::eval_tests::in_query_ref_column_tests` | `crate::query::filter::tests::eval_tests::references::in_query_ref_column_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/logical_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/predicates/logical_tests.rs` | 177 | `crate::query::filter::tests::eval_tests::logical_tests` | `crate::query::filter::tests::eval_tests::predicates::logical_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/mod.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/shared/mod.rs` | 21 | `crate::query::filter::tests::eval_tests` | `crate::query::filter::tests::eval_tests::shared` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/null_exists_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/predicates/null_exists_tests.rs` | 203 | `crate::query::filter::tests::eval_tests::null_exists_tests` | `crate::query::filter::tests::eval_tests::predicates::null_exists_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/numeric_cmp_cross_path_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/comparisons/numeric_cmp_cross_path_tests.rs` | 408 | `crate::query::filter::tests::eval_tests::numeric_cmp_cross_path_tests` | `crate::query::filter::tests::eval_tests::comparisons::numeric_cmp_cross_path_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/query_ref_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/references/query_ref_tests.rs` | 94 | `crate::query::filter::tests::eval_tests::query_ref_tests` | `crate::query::filter::tests::eval_tests::references::query_ref_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/resolve_compare_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/comparisons/resolve_compare_tests.rs` | 120 | `crate::query::filter::tests::eval_tests::resolve_compare_tests` | `crate::query::filter::tests::eval_tests::comparisons::resolve_compare_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/string_regex_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/predicates/string_regex_tests.rs` | 195 | `crate::query::filter::tests::eval_tests::string_regex_tests` | `crate::query::filter::tests::eval_tests::predicates::string_regex_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/u64_big_filter_match_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/predicates/u64_big_filter_match_tests.rs` | 262 | `crate::query::filter::tests::eval_tests::u64_big_filter_match_tests` | `crate::query::filter::tests::eval_tests::predicates::u64_big_filter_match_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/value_compare_null_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/comparisons/value_compare_null_tests.rs` | 280 | `crate::query::filter::tests::eval_tests::value_compare_null_tests` | `crate::query::filter::tests::eval_tests::comparisons::value_compare_null_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/field_path_cache_tests.rs` | `crates/shamir-engine/src/query/filter/tests/evaluation/field_path_cache_tests.rs` | 431 | `crate::query::filter::tests::field_path_cache_tests` | `crate::query::filter::tests::evaluation::field_path_cache_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/filter_tests.rs` | `crates/shamir-engine/src/query/filter/tests/evaluation/filter_tests.rs` | 642 | `crate::query::filter::tests::filter_tests` | `crate::query::filter::tests::evaluation::filter_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/local_cond_cache_tests.rs` | `crates/shamir-engine/src/query/filter/tests/caches/local_cond_cache_tests.rs` | 236 | `crate::query::filter::tests::local_cond_cache_tests` | `crate::query::filter::tests::caches::local_cond_cache_tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/mod.rs` | `crates/shamir-engine/src/query/filter/tests/mod.rs` | 9 | `crate::query::filter::tests` | `crate::query::filter::tests` | Q |
| `crates/shamir-engine/src/query/filter/tests/query_ref_cache_tests.rs` | `crates/shamir-engine/src/query/filter/tests/caches/query_ref_cache_tests.rs` | 440 | `crate::query::filter::tests::query_ref_cache_tests` | `crate::query::filter::tests::caches::query_ref_cache_tests` | Q |
| `crates/shamir-engine/src/query/mod.rs` | `crates/shamir-engine/src/query/mod.rs` | 22 | `crate::query` | `crate::query` | Q |
| `crates/shamir-engine/src/query/read/README.md` | `crates/shamir-engine/src/query/read/docs/README.md` | 134 | `not a Rust item` | `crate::query::read::docs::README.md` | Q |
| `crates/shamir-engine/src/query/read/aggregate.rs` | `crates/shamir-engine/src/query/read/aggregation/accumulator.rs` | 910 | `crate::query::read::aggregate` | `crate::query::read::aggregation::accumulator` | Q |
| `crates/shamir-engine/src/query/read/exec.rs` | `crates/shamir-engine/src/query/read/exec.rs` | 343 | `crate::query::read::exec` | `crate::query::read::exec` | Q |
| `crates/shamir-engine/src/query/read/hashable_query_value.rs` | `crates/shamir-engine/src/query/read/projection/hashable_query_value.rs` | 248 | `crate::query::read::hashable_query_value` | `crate::query::read::projection::hashable_query_value` | Q |
| `crates/shamir-engine/src/query/read/mod.rs` | `crates/shamir-engine/src/query/read/mod.rs` | 26 | `crate::query::read` | `crate::query::read` | Q |
| `crates/shamir-engine/src/query/read/order.rs` | `crates/shamir-engine/src/query/read/projection/order.rs` | 534 | `crate::query::read::order` | `crate::query::read::projection::order` | Q |
| `crates/shamir-engine/src/query/read/select_projection.rs` | `crates/shamir-engine/src/query/read/projection/select_projection.rs` | 293 | `crate::query::read::select_projection` | `crate::query::read::projection::select_projection` | Q |
| `crates/shamir-engine/src/query/read/tests/aggregate_expression_execution_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/legacy_and_caps/aggregate_expression_execution_tests.rs` | 275 | `crate::query::read::tests::aggregate_expression_execution_tests` | `crate::query::read::tests::aggregation::legacy_and_caps::aggregate_expression_execution_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/aggregate_expression_input_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/legacy_and_caps/aggregate_expression_input_tests.rs` | 280 | `crate::query::read::tests::aggregate_expression_input_tests` | `crate::query::read::tests::aggregation::legacy_and_caps::aggregate_expression_input_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/aggregate_null_input_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/legacy_and_caps/aggregate_null_input_tests.rs` | 103 | `crate::query::read::tests::aggregate_null_input_tests` | `crate::query::read::tests::aggregation::legacy_and_caps::aggregate_null_input_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/select_tests.rs` | 263 | `crate::query::read::tests::exec_tests` | `crate::query::read::tests::query_results::select_order_page::select_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/hashable_query_value_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/legacy_and_caps/hashable_query_value_tests.rs` | 129 | `crate::query::read::tests::hashable_query_value_tests` | `crate::query::read::tests::aggregation::legacy_and_caps::hashable_query_value_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/mod.rs` | `crates/shamir-engine/src/query/read/tests/mod.rs` | 12 | `crate::query::read::tests` | `crate::query::read::tests` | Q |
| `crates/shamir-engine/src/query/read/tests/pagination_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/pagination_tests.rs` | 200 | `crate::query::read::tests::pagination_tests` | `crate::query::read::tests::query_results::pagination_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/qv_order_tests.rs` | 555 | `crate::query::read::tests::qv_postprocess_tests` | `crate::query::read::tests::query_results::select_order_page::qv_order_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/select_projection_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_projection_tests.rs` | 729 | `crate::query::read::tests::select_projection_tests` | `crate::query::read::tests::query_results::select_projection_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/select_refs_params_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_refs_params_tests.rs` | 208 | `crate::query::read::tests::select_refs_params_tests` | `crate::query::read::tests::query_results::select_refs_params_tests` | Q |
| `crates/shamir-engine/src/query/read/tests/string_agg_cap_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/legacy_and_caps/string_agg_cap_tests.rs` | 167 | `crate::query::read::tests::string_agg_cap_tests` | `crate::query::read::tests::aggregation::legacy_and_caps::string_agg_cap_tests` | Q |
| `crates/shamir-engine/src/query/write/README.md` | `crates/shamir-engine/src/query/write/README.md` | 131 | `not a Rust item` | `crate::query::write::README.md` | Q |
| `crates/shamir-engine/src/query/write/mod.rs` | `crates/shamir-engine/src/query/write/mod.rs` | 13 | `crate::query::write` | `crate::query::write` | Q |
| `crates/shamir-engine/src/query/write/tests/mod.rs` | `crates/shamir-engine/src/query/write/tests/mod.rs` | 1 | `crate::query::write::tests` | `crate::query::write::tests` | Q |
| `crates/shamir-engine/src/query/write/tests/write_op_tests.rs` | `crates/shamir-engine/src/query/write/tests/write_op_tests.rs` | 556 | `crate::query::write::tests::write_op_tests` | `crate::query::write::tests::write_op_tests` | Q |
| `crates/shamir-engine/src/validator/decode.rs` | `crates/shamir-engine/src/validator/codecs/decode.rs` | 122 | `crate::validator::decode` | `crate::validator::codecs::decode` | Q |
| `crates/shamir-engine/src/validator/encode.rs` | `crates/shamir-engine/src/validator/codecs/encode.rs` | 109 | `crate::validator::encode` | `crate::validator::codecs::encode` | Q |
| `crates/shamir-engine/src/validator/mod.rs` | `crates/shamir-engine/src/validator/mod.rs` | 54 | `crate::validator` | `crate::validator` | Q |
| `crates/shamir-engine/src/validator/native_adapter.rs` | `crates/shamir-engine/src/validator/codecs/native_adapter.rs` | 71 | `crate::validator::native_adapter` | `crate::validator::codecs::native_adapter` | Q |
| `crates/shamir-engine/src/validator/native_record_validator.rs` | `crates/shamir-engine/src/validator/execution/native_record_validator.rs` | 58 | `crate::validator::native_record_validator` | `crate::validator::execution::native_record_validator` | Q |
| `crates/shamir-engine/src/validator/persisted_validators.rs` | `crates/shamir-engine/src/validator/registry/persisted_validators.rs` | 9 | `crate::validator::persisted_validators` | `crate::validator::registry::persisted_validators` | Q |
| `crates/shamir-engine/src/validator/persistence.rs` | `crates/shamir-engine/src/validator/registry/persistence.rs` | 54 | `crate::validator::persistence` | `crate::validator::registry::persistence` | Q |
| `crates/shamir-engine/src/validator/query_value_conv.rs` | `crates/shamir-engine/src/validator/codecs/query_value_conv.rs` | 61 | `crate::validator::query_value_conv` | `crate::validator::codecs::query_value_conv` | Q |
| `crates/shamir-engine/src/validator/record_fields.rs` | `crates/shamir-engine/src/validator/execution/record_fields.rs` | 287 | `crate::validator::record_fields` | `crate::validator::execution::record_fields` | Q |
| `crates/shamir-engine/src/validator/record_validator.rs` | `crates/shamir-engine/src/validator/execution/record_validator.rs` | 219 | `crate::validator::record_validator` | `crate::validator::execution::record_validator` | Q |
| `crates/shamir-engine/src/validator/registry.rs` | `crates/shamir-engine/src/validator/registry/registry.rs` | 335 | `crate::validator::registry` | `crate::validator::registry::registry` | Q |
| `crates/shamir-engine/src/validator/schema/constraints.rs` | `crates/shamir-engine/src/validator/schema/rules/constraints.rs` | 119 | `crate::validator::schema::constraints` | `crate::validator::schema::rules::constraints` | Q |
| `crates/shamir-engine/src/validator/schema/cross_field.rs` | `crates/shamir-engine/src/validator/schema/rules/cross_field.rs` | 131 | `crate::validator::schema::cross_field` | `crate::validator::schema::rules::cross_field` | Q |
| `crates/shamir-engine/src/validator/schema/field_rule.rs` | `crates/shamir-engine/src/validator/schema/rules/field_rule.rs` | 558 | `crate::validator::schema::field_rule` | `crate::validator::schema::rules::field_rule` | Q |
| `crates/shamir-engine/src/validator/schema/foreign_key.rs` | `crates/shamir-engine/src/validator/schema/foreign_key.rs` | 106 | `crate::validator::schema::foreign_key` | `crate::validator::schema::foreign_key` | Q |
| `crates/shamir-engine/src/validator/schema/format.rs` | `crates/shamir-engine/src/validator/schema/rules/format.rs` | 156 | `crate::validator::schema::format` | `crate::validator::schema::rules::format` | Q |
| `crates/shamir-engine/src/validator/schema/mod.rs` | `crates/shamir-engine/src/validator/schema/mod.rs` | 28 | `crate::validator::schema` | `crate::validator::schema` | Q |
| `crates/shamir-engine/src/validator/schema/one_of_set.rs` | `crates/shamir-engine/src/validator/schema/rules/one_of_set.rs` | 92 | `crate::validator::schema::one_of_set` | `crate::validator::schema::rules::one_of_set` | Q |
| `crates/shamir-engine/src/validator/schema/rule_builder.rs` | `crates/shamir-engine/src/validator/schema/rule_builder.rs` | 281 | `crate::validator::schema::rule_builder` | `crate::validator::schema::rule_builder` | Q |
| `crates/shamir-engine/src/validator/schema/schema_validator.rs` | `crates/shamir-engine/src/validator/schema/schema_validator.rs` | 417 | `crate::validator::schema::schema_validator` | `crate::validator::schema::schema_validator` | Q |
| `crates/shamir-engine/src/validator/schema/tests/builder_tests.rs` | `crates/shamir-engine/src/validator/schema/tests/schema_rules/builder_tests.rs` | 133 | `crate::validator::schema::tests::builder_tests` | `crate::validator::schema::tests::schema_rules::builder_tests` | Q |
| `crates/shamir-engine/src/validator/schema/tests/check_tests.rs` | `crates/shamir-engine/src/validator/schema/tests/schema_rules/check_tests.rs` | 598 | `crate::validator::schema::tests::check_tests` | `crate::validator::schema::tests::schema_rules::check_tests` | Q |
| `crates/shamir-engine/src/validator/schema/tests/foreign_key_tests.rs` | `crates/shamir-engine/src/validator/schema/tests/foreign_key_tests.rs` | 113 | `crate::validator::schema::tests::foreign_key_tests` | `crate::validator::schema::tests::foreign_key_tests` | Q |
| `crates/shamir-engine/src/validator/schema/tests/mod.rs` | `crates/shamir-engine/src/validator/schema/tests/mod.rs` | 6 | `crate::validator::schema::tests` | `crate::validator::schema::tests` | Q |
| `crates/shamir-engine/src/validator/schema/tests/phase_b_tests.rs` | `crates/shamir-engine/src/validator/schema/tests/schema_rules/phase_b_tests.rs` | 465 | `crate::validator::schema::tests::phase_b_tests` | `crate::validator::schema::tests::schema_rules::phase_b_tests` | Q |
| `crates/shamir-engine/src/validator/schema/tests/schema_validator_tests.rs` | `crates/shamir-engine/src/validator/schema/tests/schema_rules/schema_validator_tests.rs` | 465 | `crate::validator::schema::tests::schema_validator_tests` | `crate::validator::schema::tests::schema_rules::schema_validator_tests` | Q |
| `crates/shamir-engine/src/validator/schema/tests/unique_tests.rs` | `crates/shamir-engine/src/validator/schema/tests/schema_rules/unique_tests.rs` | 108 | `crate::validator::schema::tests::unique_tests` | `crate::validator::schema::tests::schema_rules::unique_tests` | Q |
| `crates/shamir-engine/src/validator/schema/type_tag.rs` | `crates/shamir-engine/src/validator/schema/rules/type_tag.rs` | 58 | `crate::validator::schema::type_tag` | `crate::validator::schema::rules::type_tag` | Q |
| `crates/shamir-engine/src/validator/tests/binding_tests.rs` | `crates/shamir-engine/src/validator/tests/execution/binding_tests.rs` | 58 | `crate::validator::tests::binding_tests` | `crate::validator::tests::execution::binding_tests` | Q |
| `crates/shamir-engine/src/validator/tests/decode_tests.rs` | `crates/shamir-engine/src/validator/tests/codecs/decode_tests.rs` | 233 | `crate::validator::tests::decode_tests` | `crate::validator::tests::codecs::decode_tests` | Q |
| `crates/shamir-engine/src/validator/tests/encode_tests.rs` | `crates/shamir-engine/src/validator/tests/codecs/encode_tests.rs` | 62 | `crate::validator::tests::encode_tests` | `crate::validator::tests::codecs::encode_tests` | Q |
| `crates/shamir-engine/src/validator/tests/mod.rs` | `crates/shamir-engine/src/validator/tests/mod.rs` | 12 | `crate::validator::tests` | `crate::validator::tests` | Q |
| `crates/shamir-engine/src/validator/tests/native_adapter_tests.rs` | `crates/shamir-engine/src/validator/tests/codecs/native_adapter_tests.rs` | 106 | `crate::validator::tests::native_adapter_tests` | `crate::validator::tests::codecs::native_adapter_tests` | Q |
| `crates/shamir-engine/src/validator/tests/persistence_tests.rs` | `crates/shamir-engine/src/validator/tests/codecs/persistence_tests.rs` | 55 | `crate::validator::tests::persistence_tests` | `crate::validator::tests::codecs::persistence_tests` | Q |
| `crates/shamir-engine/src/validator/tests/query_value_conv_tests.rs` | `crates/shamir-engine/src/validator/tests/codecs/query_value_conv_tests.rs` | 234 | `crate::validator::tests::query_value_conv_tests` | `crate::validator::tests::codecs::query_value_conv_tests` | Q |
| `crates/shamir-engine/src/validator/tests/record_fields_tests.rs` | `crates/shamir-engine/src/validator/tests/execution/record_fields_tests.rs` | 290 | `crate::validator::tests::record_fields_tests` | `crate::validator::tests::execution::record_fields_tests` | Q |
| `crates/shamir-engine/src/validator/tests/registry_atomic_tests.rs` | `crates/shamir-engine/src/validator/tests/registry/registry_atomic_tests.rs` | 349 | `crate::validator::tests::registry_atomic_tests` | `crate::validator::tests::registry::registry_atomic_tests` | Q |
| `crates/shamir-engine/src/validator/tests/registry_tests.rs` | `crates/shamir-engine/src/validator/tests/registry/registry_tests.rs` | 133 | `crate::validator::tests::registry_tests` | `crate::validator::tests::registry::registry_tests` | Q |
| `crates/shamir-engine/src/validator/tests/run_validators_tests.rs` | `crates/shamir-engine/src/validator/tests/execution/run_validators_tests.rs` | 407 | `crate::validator::tests::run_validators_tests` | `crate::validator::tests::execution::run_validators_tests` | Q |
| `crates/shamir-engine/src/validator/tests/validator_actor_tests.rs` | `crates/shamir-engine/src/validator/tests/execution/validator_actor_tests.rs` | 179 | `crate::validator::tests::validator_actor_tests` | `crate::validator::tests::execution::validator_actor_tests` | Q |
| `crates/shamir-engine/src/validator/tests/validator_db_tests.rs` | `crates/shamir-engine/src/validator/tests/registry/validator_db_tests.rs` | 925 | `crate::validator::tests::validator_db_tests` | `crate::validator::tests::registry::validator_db_tests` | Q |
| `crates/shamir-engine/src/validator/transform_spec.rs` | `crates/shamir-engine/src/validator/execution/transform_spec.rs` | 35 | `crate::validator::transform_spec` | `crate::validator::execution::transform_spec` | Q |
| `crates/shamir-engine/src/validator/validation_outcome.rs` | `crates/shamir-engine/src/validator/execution/validation_outcome.rs` | 56 | `crate::validator::validation_outcome` | `crate::validator::execution::validation_outcome` | Q |
| `crates/shamir-engine/src/validator/validator_binding.rs` | `crates/shamir-engine/src/validator/registry/validator_binding.rs` | 18 | `crate::validator::validator_binding` | `crate::validator::registry::validator_binding` | Q |
| `crates/shamir-engine/src/validator/validator_db.rs` | `crates/shamir-engine/src/validator/registry/validator_db.rs` | 522 | `crate::validator::validator_db` | `crate::validator::registry::validator_db` | Q |
| `crates/shamir-engine/src/validator/wasm_record_validator.rs` | `crates/shamir-engine/src/validator/execution/wasm_record_validator.rs` | 98 | `crate::validator::wasm_record_validator` | `crate::validator::execution::wasm_record_validator` | Q |
| `crates/shamir-engine/tests/clean_horizon/admitted_field_id.rs` | `crates/shamir-engine/tests/clean_horizon/admitted_field_id.rs` | 23 | `integration target crate::crash_recovery` | `integration target root` | Q |
| `crates/shamir-engine/tests/clean_horizon/crash_matrix.rs` | `crates/shamir-engine/tests/clean_horizon/crash_matrix.rs` | 40 | `integration target crate::crash_recovery` | `integration target root` | Q |
| `crates/shamir-engine/tests/crash_recovery.rs` | `crates/shamir-engine/tests/crash_recovery.rs` | 494 | `integration target crate::crash_recovery` | `integration target root` | Q |
| `crates/shamir-engine/tests/flush_persist_once_tests.rs` | `crates/shamir-engine/tests/flush_persist_once_tests.rs` | 101 | `integration target crate::crash_recovery` | `integration target root` | Q |
| `crates/shamir-engine/tests/overlay_depth_probe.rs` | `crates/shamir-engine/tests/overlay_depth_probe.rs` | 245 | `integration target crate::crash_recovery` | `integration target root` | Q |
| `crates/shamir-engine/tests/prealloc_caps.rs` | `crates/shamir-engine/tests/prealloc_caps.rs` | 165 | `integration target crate::crash_recovery` | `integration target root` | Q |
| `crates/shamir-engine/tests/string_agg_caps.rs` | `crates/shamir-engine/tests/string_agg_caps.rs` | 150 | `integration target crate::crash_recovery` | `integration target root` | Q |

## Oversized source splits

Ranges are inclusive original-source line spans. Topics follow observed item/function seams; first emitted artifact retains the source header and common imports, with per-output copied header/import budget conservatively added to expected size. Each source is divided among complete semantic units; do not cut through functions/items. For `query_runner.rs`, split requires extracting control-flow arms into operation helpers, not copying arbitrary equal line slices. The primary artifact keeps the existing module anchor.

| Source | Destination | Start | End | Expected lines | Topic | Stage |
|---|---|---:|---:|---:|---|---|
| `crates/shamir-engine/src/query/batch/fk_actions.rs` | `crates/shamir-engine/src/query/batch/foreign_key/cascade/planning.rs` | 1 | 572 | 607 | cascade plan types, guards, setup and direct-child planning | Q |
| `crates/shamir-engine/src/query/batch/fk_actions.rs` | `crates/shamir-engine/src/query/batch/foreign_key/cascade/grandchildren.rs` | 573 | 904 | 357 | recursive cascade traversal over explicit descendant row ids | Q |
| `crates/shamir-engine/src/query/batch/fk_actions.rs` | `crates/shamir-engine/src/query/batch/foreign_key/cascade/apply.rs` | 905 | 1055 | 176 | ordered transactional cascade/set-null application | Q |
| `crates/shamir-engine/src/query/batch/fk_actions.rs` | `crates/shamir-engine/src/query/batch/foreign_key/cascade/discovery.rs` | 1056 | 1207 | 177 | reverse-FK discovery and tx-aware parent value scan | Q |
| `crates/shamir-engine/src/query/batch/fk_actions.rs` | `crates/shamir-engine/src/query/batch/foreign_key/cascade/row_matching.rs` | 1208 | 1438 | 259 | decode, compare and indexed candidate matching | Q |
| `crates/shamir-engine/src/query/batch/fk_on_update.rs` | `crates/shamir-engine/src/query/batch/foreign_key/update/update_plan.rs` | 1 | 536 | 571 | on-update types and action planning, cascade/restrict decisions | Q |
| `crates/shamir-engine/src/query/batch/fk_on_update.rs` | `crates/shamir-engine/src/query/batch/foreign_key/update/update_apply.rs` | 537 | 770 | 259 | child field mutation and nullability enforcement | Q |
| `crates/shamir-engine/src/query/batch/fk_on_update.rs` | `crates/shamir-engine/src/query/batch/foreign_key/update/update_discovery.rs` | 771 | 1048 | 303 | reverse-FK discovery, tx-aware parent scan and child probe | Q |
| `crates/shamir-engine/src/query/batch/fk_on_update.rs` | `crates/shamir-engine/src/query/batch/foreign_key/update/update_row_matching.rs` | 1049 | 1353 | 333 | staged-value matching, decoding, index and scalar conversion | Q |
| `crates/shamir-engine/src/query/read/aggregate.rs` | `crates/shamir-engine/src/query/read/aggregation/accumulator.rs` | 1 | 865 | 910 | owned group keys and accumulator state machine with input resolution | Q |
| `crates/shamir-engine/src/query/read/aggregate.rs` | `crates/shamir-engine/src/query/read/aggregation/validation.rs` | 866 | 935 | 90 | aggregate argument and distinct validation | Q |
| `crates/shamir-engine/src/query/read/aggregate.rs` | `crates/shamir-engine/src/query/read/aggregation/function_inputs.rs` | 936 | 1226 | 321 | capped aggregator, binding resolution and strict input stepping | Q |
| `crates/shamir-engine/src/query/read/aggregate.rs` | `crates/shamir-engine/src/query/read/aggregation/group_projection.rs` | 1227 | 1577 | 386 | group aggregate object and row-view projection | Q |
| `crates/shamir-engine/src/query/read/aggregate.rs` | `crates/shamir-engine/src/query/read/aggregation/group_execution.rs` | 1578 | 1779 | 227 | group construction, HAVING and aggregate-all entry points | Q |
| `crates/shamir-engine/src/query/filter/filter_node.rs` | `crates/shamir-engine/src/query/filter/evaluation/filter_node/mod.rs` | 1 | 1099 | 785 | FilterNode parent: header plus CompareOp/FilterNode/matches; helper declarations are extracted to descendant membership module using noncontiguous retained ranges | Q |
| `crates/shamir-engine/src/query/filter/filter_node.rs` | `crates/shamir-engine/src/query/filter/evaluation/filter_node/membership.rs` | 51 | 364 | 344 | membership probe and collection-matching helper definitions extracted intact as FilterNode descendants | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/delete/cascade_tests.rs` | 1 | 550 | 580 | delete cascade topology, cycle guard and plan behavior | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/delete/set_null_tests.rs` | 551 | 927 | 407 | SET NULL, nullability, and cross-type values | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/delete/self_ref_tests.rs` | 928 | 1037 | 140 | self-referential SET NULL | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/delete/staged_tx_tests.rs` | 1038 | 1333 | 326 | transactional overlays and staged writes | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/delete/index_fast_path_tests.rs` | 1334 | 1688 | 390 | index/scan parity and staged-write fallback | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/update/on_update_core_tests.rs` | 1 | 712 | 742 | core restrict/cascade/set-null fanout and no-op behavior | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/update/on_update_no_action_tests.rs` | 713 | 1264 | 582 | NoAction, equal-value, multi-table and non-regression cases | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/update/on_update_coercion_tests.rs` | 1265 | 1336 | 97 | Int/F64 coercion regression | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/update/on_update_self_ref_tests.rs` | 1337 | 1910 | 604 | self-referential and same-tx update/delete ordering | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/update/on_update_index_tests.rs` | 1911 | 2228 | 348 | index parity and multi-parent restrict fan-out | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_actions/update/on_update_cache_tests.rs` | 2229 | 2481 | 283 | reverse-cache no-op gate and resolver counting | Q |
| `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations/for_each_core_tests.rs` | 1 | 712 | 747 | iteration cardinality, bind and first-failure transactional semantics | Q |
| `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations/for_each_atomic_tests.rs` | 713 | 928 | 246 | partial rollback and full commit visibility | Q |
| `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations/for_each_sources_tests.rs` | 929 | 1294 | 401 | column, function and eager over-source resolution | Q |
| `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations/for_each_gates_tests.rs` | 1295 | 1766 | 507 | autocommit stop-first, authorization gates, when and user scalar | Q |
| `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations/for_each_plan_tests.rs` | 1767 | 2151 | 420 | body plan validation once before iterations | Q |
| `crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/sub_batch/sub_batch_execution_tests.rs` | 1 | 790 | 825 | sub-batch execution, bind and transaction atomicity | Q |
| `crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs` | `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/sub_batch/sub_batch_param_tests.rs` | 791 | 1509 | 754 | unbound parameters, nested write substitutions and scalar binding | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/integration/implicit_delete_races.rs` | 1 | 696 | 731 | implicit delete SSI race closure and never-interned field | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/integration/implicit_update_races.rs` | 697 | 1147 | 486 | implicit on-update race, retry outcomes and NoAction regression | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/integration/explicit_delete_races.rs` | 1 | 524 | 559 | explicit Snapshot delete RI-barrier race and quiescent controls | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/integration/explicit_update_races.rs` | 525 | 884 | 395 | explicit Snapshot on-update race and quiescent control | Q |
| `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs` | `crates/shamir-engine/src/query/batch/tests/referential_races/integration/commit_window_races.rs` | 885 | 1341 | 492 | post-validation commit-lock window serialization tests | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/collections/membership_literals_tests.rs` | 1 | 631 | 661 | IN/NOT IN and contains list/set membership/parity | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/collections/numeric_coercion_tests.rs` | 632 | 837 | 231 | numeric cross-type membership coercion | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/collections/range_and_container_tests.rs` | 838 | 1101 | 289 | BETWEEN and container-field semantics | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs` | `crates/shamir-engine/src/query/filter/tests/eval_tests/collections/binary_and_large_set_tests.rs` | 1102 | 1318 | 242 | binary matching, large set boundary and absent/null behavior | Q |
| `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/select_tests.rs` | 1 | 233 | 263 | select projection basics and scalar function outputs | Q |
| `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/grouping_tests.rs` | 234 | 435 | 232 | group-by, having and aggregate-all basics | Q |
| `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/ordering_tests.rs` | 436 | 588 | 178 | order-by comparison and empty/single-row semantics | Q |
| `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/pagination_distinct_tests.rs` | 589 | 697 | 134 | pagination and distinct behavior | Q |
| `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/numeric_and_functions/function_aggregate_tests.rs` | 698 | 1022 | 355 | function aggregates, parameter validation and user scalar paths | Q |
| `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/numeric_and_functions/distinct_aggregate_tests.rs` | 1023 | 1222 | 225 | aggregate distinct wrappers and validation behavior | Q |
| `crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/qv_order_tests.rs` | 1 | 525 | 555 | QueryValue order, null, binary and composition semantics | Q |
| `crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs` | `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/qv_select_tests.rs` | 526 | 661 | 166 | QueryValue select and projection semantics | Q |
| `crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/numeric_and_functions/qv_aggregate_tests.rs` | 662 | 1207 | 576 | QueryValue aggregate results and decimal/numeric ordering | Q |
| `crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs` | `crates/shamir-engine/src/query/read/tests/aggregation/numeric_and_functions/qv_exact_sum_tests.rs` | 1208 | 1354 | 172 | exact integer overflow and float-lane transitions | Q |
| `crates/shamir-engine/benches/tx_concurrent.rs` | `crates/shamir-engine/benches/storage_concurrency/transactions/tx_concurrent.rs` | 1 | 238 | 238 | primary Cargo target prelude, adapters and main entry anchor | Q |
| `crates/shamir-engine/benches/tx_concurrent.rs` | `crates/shamir-engine/benches/storage_concurrency/transactions/tx_concurrent_parts/workloads.rs` | 239 | 791 | 583 | transaction concurrency benchmark groups: key contention, lock acquisition and wait order | Q |
| `crates/shamir-engine/benches/tx_concurrent.rs` | `crates/shamir-engine/benches/storage_concurrency/transactions/tx_concurrent_parts/fk_child_workload.rs` | 792 | 1013 | 247 | FK-child commit-footprint concurrency workload and result reporting | Q |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | `crates/shamir-engine/src/query/batch/query/query_runner.rs` | 1 | 618 | 663 | QueryRunner support types, skip semantics, FK hooks, retries and type definition | Q |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | `crates/shamir-engine/src/query/batch/query/query_runner/nested.rs` | 619 | 1082 | 509 | sub-batch and ForEach recursion, bind resolution, iteration gates and result accounting | Q |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | `crates/shamir-engine/src/query/batch/query/query_runner/reads.rs` | 1083 | 1297 | 250 | admin/call/subscription control and read dispatch path | Q |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | `crates/shamir-engine/src/query/batch/query/query_runner/writes.rs` | 1298 | 1934 | 682 | insert/update/delete/set execution; retain transaction and retry scopes with each arm | Q |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | `crates/shamir-engine/src/query/batch/query/query_runner/test_adapter.rs` | 1935 | 2006 | 97 | cfg(test) execute_single adapter and subscription filter validator | Q |
| `crates/shamir-engine/tests/crash_recovery.rs` | `crates/shamir-engine/tests/crash_recovery.rs` | 1 | 494 | 494 | primary transaction crash target and child dispatch anchor | Q |
| `crates/shamir-engine/tests/crash_recovery.rs` | `crates/shamir-engine/tests/crash_recovery/multi_table_drain.rs` | 495 | 963 | 504 | multi-table, drain/truncate crash recovery scenarios | Q |
| `crates/shamir-engine/tests/crash_recovery.rs` | `crates/shamir-engine/tests/crash_recovery/write_to_data.rs` | 964 | 1177 | 239 | WAL-to-data mode crash and recovery scenarios | Q |
| `crates/shamir-engine/tests/crash_recovery.rs` | `crates/shamir-engine/tests/crash_recovery/vector_delta.rs` | 1178 | 1471 | 319 | vector index delta crash/reopen scenarios | Q |
| `crates/shamir-engine/tests/crash_recovery.rs` | `crates/shamir-engine/tests/crash_recovery/history_seed.rs` | 1472 | 1651 | 205 | history seed failure and successful reopen scenarios | Q |

## New module manifests

| Destination | Projected lines | Kind | Stage |
|---|---:|---|---|
| `crates/shamir-engine/benches/backend/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/durability/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/query_reads/filtering/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/query_reads/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/query_reads/ordering/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/query_reads/selection/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/query_writes/batch_ops/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/query_writes/constraints/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/query_writes/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/storage_concurrency/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/storage_concurrency/transactions/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/storage_concurrency/transactions/tx_concurrent_parts/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/benches/vector_search/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/api/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/foreign_key/cascade/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/foreign_key/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/foreign_key/restrict/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/foreign_key/update/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/query/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/query/query_runner/nested.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/query/query_runner/reads.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/query/query_runner/test_adapter.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/query/query_runner/writes.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/query/support/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/runtime/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/executor/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/basic/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/queries/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/shared/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/transactions/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/limits/control/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/limits/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/limits/result_budgets/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/limits/security/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/core/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/sub_batch/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/plan_refs/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/referential_actions/core/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/referential_actions/delete/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/referential_actions/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/referential_actions/update/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/referential_races/cache/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/referential_races/integration/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/batch/tests/referential_races/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/caches/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/evaluation/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/evaluation/planning/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/evaluation/runtime/core/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/evaluation/runtime/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/evaluation/semantics/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/caches/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/collections/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/comparisons/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/expressions/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/predicates/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/references/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/shared/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/filter/tests/evaluation/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/read/aggregation/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/read/projection/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/read/tests/aggregation/legacy_and_caps/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/read/tests/aggregation/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/read/tests/aggregation/numeric_and_functions/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/read/tests/query_results/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/query/read/tests/query_results/select_order_page/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/codecs/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/execution/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/registry/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/runtime/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/schema/rules/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/schema/tests/schema_rules/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/tests/codecs/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/tests/execution/mod.rs` | 25 | mod.rs | Q |
| `crates/shamir-engine/src/validator/tests/registry/mod.rs` | 25 | mod.rs | Q |

## Actual #[path] mounts (OBSERVED)

| Source | Line | Text / meaning |
|---|---:|---|
| `crates/shamir-engine/tests/crash_recovery.rs` | 41 | `#[path = "clean_horizon/admitted_field_id.rs"]` |
| `crates/shamir-engine/tests/crash_recovery.rs` | 43 | `#[path = "clean_horizon/crash_matrix.rs"]` |

## Restricted visibility sites (OBSERVED)

| Source | Line | Text / meaning |
|---|---:|---|
| `crates/shamir-engine/src/query/batch/authorized.rs` | 174 | `pub(super) fn with_deadline_cap_at_for_test(self, now: Instant, cap: Duration) -> Self {` |
| `crates/shamir-engine/src/query/batch/authorized.rs` | 184 | `pub(super) fn root_scope(&self) -> ExecScope {` |
| `crates/shamir-engine/src/query/batch/authorized.rs` | 216 | `/// in this module. `pub(super)` — only `execute_batch`/` |
| `crates/shamir-engine/src/query/batch/authorized.rs` | 219 | `pub(super) fn into_parts(self) -> (&'a BatchRequest, Actor, &'a str) {` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 156 | `pub(super) async fn plan_and_validate_batch(` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 275 | `pub(super) fn execute_batch_impl<'a>(` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 382 | `pub(super) fn execute_planned_batch_impl<'a>(` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 541 | `pub(super) async fn execute_plan_impl(` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 641 | `pub(super) async fn execute_plan_tx(` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 670 | `pub(super) async fn execute_plan_tx_impl(` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 763 | `pub(super) async fn execute_transactional_impl(` |
| `crates/shamir-engine/src/query/batch/batch_execute.rs` | 940 | `pub(super) fn filter_results(` |
| `crates/shamir-engine/src/query/batch/batch_validate.rs` | 15 | `pub(super) async fn validate_tables(` |
| `crates/shamir-engine/src/query/batch/batch_validate.rs` | 59 | `pub(super) fn validate_pagination(` |
| `crates/shamir-engine/src/query/batch/batch_validate.rs` | 115 | `pub(super) fn tables_created_in_batch(` |
| `crates/shamir-engine/src/query/batch/batch_validate.rs` | 170 | `pub(super) fn validate_filter_depth(queries: &TMap<String, QueryEntry>) -> Result<(), BatchError> {` |
| `crates/shamir-engine/src/query/batch/batch_validate.rs` | 193 | `pub(super) fn validate_filter_patterns(` |
| `crates/shamir-engine/src/query/batch/exec_scope.rs` | 109 | `pub(super) fn with_deadline_cap(mut self, deadline: Instant, cap: Duration) -> Self {` |
| `crates/shamir-engine/src/query/batch/exec_scope.rs` | 120 | `pub(super) fn cap_deadline(&self) -> Option<Instant> {` |
| `crates/shamir-engine/src/query/batch/exec_scope.rs` | 126 | `pub(super) fn capped_budget(&self) -> Option<Duration> {` |
| `crates/shamir-engine/src/query/batch/exec_scope.rs` | 144 | `pub(super) fn with_read_visibility_timeout(mut self, timeout_ms: u64) -> Self {` |
| `crates/shamir-engine/src/query/batch/exec_scope.rs` | 165 | `pub(super) fn read_visibility_budget_at_for_test(&self, now: Instant) -> Duration {` |
| `crates/shamir-engine/src/query/batch/execution_deadline.rs` | 294 | `pub(super) fn remaining_at(&self, now: Instant) -> Option<Duration> {` |
| `crates/shamir-engine/src/query/batch/interactive_tx.rs` | 93 | `pub(super) async fn execute_in_open_tx_with_scope(` |
| `crates/shamir-engine/src/query/batch/param_subst.rs` | 127 | `pub(super) fn contains_param_ref(value: &QueryValue) -> bool {` |
| `crates/shamir-engine/src/query/batch/param_subst.rs` | 146 | `pub(super) enum WriteValueError {` |
| `crates/shamir-engine/src/query/batch/param_subst.rs` | 195 | `pub(super) fn resolve_write_value(` |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | 52 | `pub(super) fn build_resolved_refs(` |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | 78 | `pub(super) fn skipped_query_result() -> QueryResult {` |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | 116 | `pub(super) fn resolve_skip(` |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | 619 | `pub(super) async fn run_with_wait(` |
| `crates/shamir-engine/src/query/batch/query_runner.rs` | 1935 | `pub(super) async fn execute_single_impl(` |
| `crates/shamir-engine/src/query/batch/receipts/accounting.rs` | 11 | `pub(in crate::query::batch) fn write_result_to_query_result_with_encoding(` |
| `crates/shamir-engine/src/query/batch/receipts/accounting.rs` | 50 | `pub(in crate::query::batch) fn reserve(` |
| `crates/shamir-engine/src/query/batch/receipts/accounting.rs` | 66 | `pub(in crate::query::batch) fn receipt_size(receipt: &TransactionInfo) -> u64 {` |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 16 | `pub(in crate::query::batch) async fn scope<T>(future: impl Future<Output = T>) -> T {` |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 24 | `pub(in crate::query::batch) fn record(repo: &str, receipt: &TransactionInfo) {` |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 38 | `pub(in crate::query::batch) fn required(entry: &QueryEntry) -> Option<(u64, u64)> {` |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 55 | `pub(in crate::query::batch) fn failure(result: &QueryResult) -> Option<(u64, u64)> {` |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 82 | `pub(in crate::query::batch) fn failure_result(` |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 98 | `pub(in crate::query::batch) fn has_receipt(result: &QueryResult) -> bool {` |
| `crates/shamir-engine/src/query/batch/receipts/dependency.rs` | 119 | `pub(in crate::query::batch) fn pending(result: &QueryResult) -> Option<(u64, u64)> {` |
| `crates/shamir-engine/src/query/batch/receipts/mod.rs` | 7 | `pub(super) use dependency::{failure_result, has_receipt, required, scope};` |
| `crates/shamir-engine/src/query/batch/receipts/mod.rs` | 8 | `pub(super) use policy::{inherit, validate};` |
| `crates/shamir-engine/src/query/batch/receipts/mod.rs` | 10 | `pub(super) use accounting::{receipt_size, reserve, write_result_to_query_result_with_encoding};` |
| `crates/shamir-engine/src/query/batch/receipts/mod.rs` | 13 | `pub(super) use propagation::{attach, execute_plan, filter, stamp};` |
| `crates/shamir-engine/src/query/batch/receipts/policy.rs` | 8 | `pub(in crate::query::batch) async fn validate(` |
| `crates/shamir-engine/src/query/batch/receipts/policy.rs` | 101 | `pub(in crate::query::batch) fn inherit(` |
| `crates/shamir-engine/src/query/batch/receipts/propagation.rs` | 29 | `pub(in crate::query::batch) fn filter(` |
| `crates/shamir-engine/src/query/batch/receipts/propagation.rs` | 49 | `pub(in crate::query::batch) fn stamp(` |
| `crates/shamir-engine/src/query/batch/receipts/propagation.rs` | 68 | `pub(in crate::query::batch) fn attach(` |
| `crates/shamir-engine/src/query/batch/receipts/propagation.rs` | 83 | `pub(in crate::query::batch) async fn execute_plan(` |
| `crates/shamir-engine/src/query/batch/result_size.rs` | 9 | `pub(super) struct ResultSize;` |
| `crates/shamir-engine/src/query/batch/result_size.rs` | 19 | `pub(super) const SHELL: u64 = 24;` |
| `crates/shamir-engine/src/query/batch/result_size.rs` | 20 | `pub(super) const ENTRY: u64 = 16;` |
| `crates/shamir-engine/src/query/batch/result_size.rs` | 22 | `pub(super) fn value(value: &QueryValue) -> u64 {` |
| `crates/shamir-engine/src/query/batch/result_size.rs` | 57 | `pub(super) fn sequence(len: usize) -> u64 {` |
| `crates/shamir-engine/src/query/batch/result_size.rs` | 61 | `pub(super) fn result(result: &QueryResult) -> u64 {` |
| `crates/shamir-engine/src/query/batch/result_size.rs` | 150 | `pub(super) fn assembled(results: &TMap<String, QueryResult>) -> u64 {` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 15 | `pub(super) struct TestResolver {` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 16 | `pub(super) db: DbInstance,` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 17 | `pub(super) repo: String,` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 37 | `pub(super) async fn setup_resolver() -> TestResolver {` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 50 | `pub(super) struct TxTestResolver {` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 51 | `pub(super) repo: crate::repo::RepoInstance,` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 73 | `pub(super) fn resolver_with_user_scalar() -> ScalarResolver {` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 96 | `pub(super) struct TestResolverWithScalars {` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 97 | `pub(super) db: DbInstance,` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 98 | `pub(super) repo: String,` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 99 | `pub(super) scalars: ScalarResolver,` |
| `crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs` | 119 | `pub(super) async fn setup_resolver_with_scalars() -> TestResolverWithScalars {` |
| `crates/shamir-engine/src/query/filter/compile.rs` | 191 | `pub(super) fn build_index_expr(` |
| `crates/shamir-engine/src/query/filter/compile.rs` | 307 | `pub(super) fn compile_compare(` |
| `crates/shamir-engine/src/query/filter/field_path_cache.rs` | 47 | `pub(super) fn field_ref_key(fv: &FilterValue) -> String {` |
| `crates/shamir-engine/src/query/filter/filter_node.rs` | 358 | `pub(super) type CompactPath = SmallVec<[InternerKey; 4]>;` |
| `crates/shamir-engine/src/query/filter/fts.rs` | 6 | `pub(super) fn like_pattern_to_regex(pattern: &str, case_insensitive: bool) -> Option<Regex> {` |
| `crates/shamir-engine/src/query/filter/fts.rs` | 33 | `pub(super) fn fts_word_eq_token(word: &str, token: &str) -> bool {` |
| `crates/shamir-engine/src/query/filter/fts.rs` | 50 | `pub(super) fn fts_word_matches(word: &str, query_tokens: &[String], seen: &mut u64) -> bool {` |
| `crates/shamir-engine/src/query/filter/fts.rs` | 64 | `pub(super) fn fts_word_matches_vec(` |
| `crates/shamir-engine/src/query/filter/fts.rs` | 83 | `pub(super) fn fts_word_matches_or(word: &str, query_tokens: &[String]) -> bool {` |
| `crates/shamir-engine/src/query/filter/index_range.rs` | 13 | `pub(super) fn encode_filter_value(v: &FilterValue) -> Option<Vec<u8>> {` |
| `crates/shamir-engine/src/query/filter/index_range.rs` | 41 | `pub(super) fn predicate_bound_lower(name_interned: u64, enc: &[u8]) -> std::ops::Bound<Bytes> {` |
| `crates/shamir-engine/src/query/filter/index_range.rs` | 50 | `pub(super) fn predicate_bound_upper(name_interned: u64, enc: &[u8]) -> std::ops::Bound<Bytes> {` |
| `crates/shamir-engine/src/query/filter/index_range.rs` | 60 | `pub(super) fn predicate_bound_prefix(name_interned: u64) -> std::ops::Bound<Bytes> {` |
| `crates/shamir-engine/src/query/filter/index_range.rs` | 68 | `pub(super) fn predicate_bound_full_upper(name_interned: u64) -> std::ops::Bound<Bytes> {` |
| `crates/shamir-engine/src/query/filter/index_range.rs` | 80 | `pub(super) fn predicate_handle_one(` |
| `crates/shamir-engine/src/query/filter/query_ref_cache.rs` | 99 | `pub(super) fn query_ref_key(fv: &FilterValue) -> String {` |
| `crates/shamir-engine/src/query/filter/resolve.rs` | 67 | `pub(super) fn intern_field_path_compact(` |
| `crates/shamir-engine/src/query/filter/resolve.rs` | 736 | `pub(super) fn resolve_query_ref_value(qr: &QueryResult, path: Option<&str>) -> Option<QueryValue> {` |
| `crates/shamir-engine/src/query/filter/resolve.rs` | 799 | `pub(super) fn resolve_query_value_path<'a>(` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 7 | `pub(super) fn make_alice_record(interner: &Interner) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 19 | `pub(super) fn make_nested_record(interner: &Interner) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 33 | `pub(super) fn make_nullable_record(interner: &Interner) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 43 | `pub(super) fn make_date_record(interner: &Interner) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 53 | `pub(super) fn make_list_record(interner: &Interner) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 70 | `pub(super) fn make_set_record(interner: &Interner) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 82 | `pub(super) fn make_body_record(interner: &Interner, body: &str) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 89 | `pub(super) fn make_email_record(interner: &Interner, email: &str) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 97 | `pub(super) fn make_ab_record(interner: &Interner) -> InnerValue {` |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/helpers.rs` | 106 | `pub(super) fn empty_refs() -> TMap<String, QueryResult> {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 66 | `pub(super) enum OwnedExtreme {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 74 | `pub(super) enum OwnedScalar {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 152 | `pub(super) enum GroupKeyItem {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 166 | `pub(super) fn group_key_item_scalar(s: Option<ScalarRef<'_>>) -> GroupKeyItem {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 185 | `pub(super) fn group_key_item_complex(val: &InnerValue, interner: &Interner) -> GroupKeyItem {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 308 | `pub(super) struct AggAccum {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 316 | `pub(super) enum AggState {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 360 | `pub(super) fn new(func: AggFunc, field: &AggregateField, interner: &Interner) -> Self {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 404 | `pub(super) fn step<R: RecordRef + ?Sized>(&mut self, record: &R) {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 649 | `pub(super) fn finish(self, interner: &Interner) -> QueryValue {` |
| `crates/shamir-engine/src/query/read/aggregate.rs` | 1227 | `pub(super) fn build_aggregate_object(` |
| `crates/shamir-engine/src/query/read/hashable_query_value.rs` | 28 | `pub(super) struct HashableQueryValue<'a>(pub(super) &'a QueryValue);` |
| `crates/shamir-engine/src/query/read/hashable_query_value.rs` | 59 | `pub(super) fn hash_qv<H: std::hash::Hasher>(qv: &QueryValue, h: &mut H) {` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 24 | `pub(super) is_all: bool,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 29 | `pub(super) fields: Vec<(Option<Vec<InternerKey>>, String)>,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 33 | `pub(super) funcs: Vec<(String, FilterValue)>,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 35 | `pub(super) resolved_refs: TMap<String, QueryResult>,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 36 | `pub(super) params: TMap<String, QueryValue>,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 46 | `pub(super) funcs_cond_cache: CondCache,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 55 | `pub(super) funcs_field_path_cache: FieldPathCache,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 69 | `pub(super) funcs_query_ref_cache: QueryRefCache,` |
| `crates/shamir-engine/src/query/read/select_projection.rs` | 73 | `pub(super) scalars: ScalarResolver,` |
| `crates/shamir-engine/src/validator/registry.rs` | 79 | `pub(super) fn set_mutation_hook(&self, hook: Option<MutationHook>) {` |

## Projected parent-entry census (DECIDED destinations)

| Parent | Direct entries | Projected count |
|---|---|---:|
| `crates/shamir-engine/benches` | `backend, durability, query_reads, query_writes, storage_concurrency, vector_search` | 6 |
| `crates/shamir-engine/benches/backend` | `backend_matrix.rs, fts_indexed.rs, keyset_seek.rs, mod.rs, wasm_invoke.rs` | 5 |
| `crates/shamir-engine/benches/durability` | `drain_cost_vs_depth.rs, drain_throughput.rs, durable_concurrent_commit.rs, f78_writer_latency.rs, f80_writer_drain_overhead.rs, mod.rs, replication_apply.rs` | 7 |
| `crates/shamir-engine/benches/query_reads` | `filtering, mod.rs, ordering, selection` | 4 |
| `crates/shamir-engine/benches/query_reads/filtering` | `cond_expr_eval.rs, filter_eval.rs, mod.rs, when_skip_eval.rs` | 4 |
| `crates/shamir-engine/benches/query_reads/ordering` | `cursor_pages_depth.rs, distinct.rs, mod.rs, order_by_pipeline.rs, read_path_matrix.rs` | 5 |
| `crates/shamir-engine/benches/query_reads/selection` | `group_by_keys.rs, mod.rs, select_pipeline.rs, select_projection.rs, streaming_topk.rs` | 5 |
| `crates/shamir-engine/benches/query_writes` | `batch_ops, constraints, mod.rs` | 3 |
| `crates/shamir-engine/benches/query_writes/batch_ops` | `batch_stage_parallelism.rs, for_each_eval.rs, mod.rs, permission_check.rs` | 4 |
| `crates/shamir-engine/benches/query_writes/constraints` | `fk_cascade_index.rs, mod.rs, p1099_touched_probe.rs, p1107_stale_value_gate.rs, p1108_released_unique_growth.rs` | 5 |
| `crates/shamir-engine/benches/storage_concurrency` | `buffers_and_interner, mod.rs, transactions` | 3 |
| `crates/shamir-engine/benches/storage_concurrency/buffers_and_interner` | `interner_cold_growth.rs, interner_concurrent.rs, membuffer_concurrent.rs, membuffer_pump.rs` | 4 |
| `crates/shamir-engine/benches/storage_concurrency/transactions` | `mod.rs, tx_aware_reads.rs, tx_concurrent.rs, tx_concurrent_parts, tx_pipeline.rs` | 5 |
| `crates/shamir-engine/benches/storage_concurrency/transactions/tx_concurrent_parts` | `fk_child_workload.rs, mod.rs, workloads.rs` | 3 |
| `crates/shamir-engine/benches/vector_search` | `filtered_vector_search.rs, mod.rs, quantization_f32_vs_sq8.rs, vector_bulk_compaction.rs, vector_search.rs` | 5 |
| `crates/shamir-engine/src` | `query, validator` | 2 |
| `crates/shamir-engine/src/query` | `admin, auth, batch, filter, mod.rs, read, write` | 7 |
| `crates/shamir-engine/src/query/admin` | `README.md, docs, mod.rs` | 3 |
| `crates/shamir-engine/src/query/admin/docs` | `README.md` | 1 |
| `crates/shamir-engine/src/query/auth` | `README.md, mod.rs, session.rs, tests` | 4 |
| `crates/shamir-engine/src/query/auth/tests` | `mod.rs, session_tests.rs` | 2 |
| `crates/shamir-engine/src/query/batch` | `api, foreign_key, mod.rs, query, receipts, runtime, tests` | 7 |
| `crates/shamir-engine/src/query/batch/api` | `README.md, authorized.rs, batch_execute.rs, batch_validate.rs, executor_traits.rs, interactive_tx.rs, mod.rs` | 7 |
| `crates/shamir-engine/src/query/batch/foreign_key` | `cascade, fk_restrict.rs, mod.rs, restrict, update` | 5 |
| `crates/shamir-engine/src/query/batch/foreign_key/cascade` | `apply.rs, discovery.rs, grandchildren.rs, mod.rs, planning.rs, row_matching.rs` | 6 |
| `crates/shamir-engine/src/query/batch/foreign_key/restrict` | `mod.rs` | 1 |
| `crates/shamir-engine/src/query/batch/foreign_key/update` | `mod.rs, update_apply.rs, update_discovery.rs, update_plan.rs, update_row_matching.rs` | 5 |
| `crates/shamir-engine/src/query/batch/query` | `mod.rs, query_runner, query_runner.rs, support` | 4 |
| `crates/shamir-engine/src/query/batch/query/query_runner` | `nested.rs, reads.rs, test_adapter.rs, writes.rs` | 4 |
| `crates/shamir-engine/src/query/batch/query/support` | `mod.rs, param_subst.rs, query_value_serializer.rs` | 3 |
| `crates/shamir-engine/src/query/batch/receipts` | `accounting.rs, commit.rs, dependency.rs, mod.rs, policy.rs, propagation.rs, tests` | 7 |
| `crates/shamir-engine/src/query/batch/receipts/tests` | `batch_tests.rs, commit_error_code_tests.rs, mod.rs` | 3 |
| `crates/shamir-engine/src/query/batch/runtime` | `exec_scope.rs, execution_deadline.rs, mod.rs, op_watchdog.rs, result_size.rs` | 5 |
| `crates/shamir-engine/src/query/batch/tests` | `executor, limits, mod.rs, query_semantics, referential_actions, referential_races` | 6 |
| `crates/shamir-engine/src/query/batch/tests/executor` | `authorized_tests.rs, call_tx_tests.rs, cross_repo_guard_tests.rs, mod.rs, nested` | 5 |
| `crates/shamir-engine/src/query/batch/tests/executor/nested` | `basic, queries, shared, transactions` | 4 |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/basic` | `basic_ops_tests.rs, error_handling_tests.rs, mod.rs, permissions_tests.rs` | 4 |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/queries` | `mod.rs, query_refs_tests.rs, query_runner_tests.rs, when_skip_tests.rs, write_value_resolution_tests.rs` | 5 |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/shared` | `common.rs, mod.rs` | 2 |
| `crates/shamir-engine/src/query/batch/tests/executor/nested/transactions` | `interactive_tx_tests.rs, mod.rs, ssi_tests.rs, transactions_tests.rs` | 4 |
| `crates/shamir-engine/src/query/batch/tests/limits` | `control, mod.rs, result_budgets, rowwise_byte_budget_tests.rs, runtime_byte_budget_tests.rs, runtime_work_counter_tests.rs, security` | 7 |
| `crates/shamir-engine/src/query/batch/tests/limits/control` | `deadline_cap_tests.rs, interactive_deadline_tests.rs, limit_enforcement_tests.rs, mod.rs, watchdog_tests.rs` | 5 |
| `crates/shamir-engine/src/query/batch/tests/limits/result_budgets` | `mod.rs` | 1 |
| `crates/shamir-engine/src/query/batch/tests/limits/security` | `dos_gate_tests.rs, mod.rs` | 2 |
| `crates/shamir-engine/src/query/batch/tests/query_semantics` | `core, mod.rs, nested_batch, plan_refs` | 4 |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/core` | `filter_guard_tests.rs, index_visibility_parity_tests.rs, mod.rs, pagination_bounds_tests.rs, subscribe_guard_tests.rs` | 5 |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch` | `iterations, mod.rs, sub_batch` | 3 |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/iterations` | `for_each_atomic_tests.rs, for_each_core_tests.rs, for_each_gates_tests.rs, for_each_plan_tests.rs, for_each_sources_tests.rs, mod.rs` | 6 |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/nested_batch/sub_batch` | `mod.rs, sub_batch_execution_tests.rs, sub_batch_param_tests.rs` | 3 |
| `crates/shamir-engine/src/query/batch/tests/query_semantics/plan_refs` | `exec_scope_tests.rs, mod.rs, planner_tests.rs, query_value_serializer_tests.rs, reference_tests.rs` | 5 |
| `crates/shamir-engine/src/query/batch/tests/referential_actions` | `core, delete, mod.rs, update` | 4 |
| `crates/shamir-engine/src/query/batch/tests/referential_actions/core` | `fk_discovery_fail_closed_tests.rs, fk_fail_closed_tests.rs, fk_indexed_action_read_error_tests.rs, fk_restrict_tests.rs, mod.rs` | 5 |
| `crates/shamir-engine/src/query/batch/tests/referential_actions/delete` | `cascade_tests.rs, index_fast_path_tests.rs, mod.rs, self_ref_tests.rs, set_null_tests.rs, staged_tx_tests.rs` | 6 |
| `crates/shamir-engine/src/query/batch/tests/referential_actions/update` | `mod.rs, on_update_cache_tests.rs, on_update_coercion_tests.rs, on_update_core_tests.rs, on_update_index_tests.rs, on_update_no_action_tests.rs, on_update_self_ref_tests.rs` | 7 |
| `crates/shamir-engine/src/query/batch/tests/referential_races` | `cache, integration, mod.rs` | 3 |
| `crates/shamir-engine/src/query/batch/tests/referential_races/cache` | `fk_reverse_cache_race_tests.rs, fk_reverse_cache_tests.rs, mod.rs` | 3 |
| `crates/shamir-engine/src/query/batch/tests/referential_races/integration` | `commit_window_races.rs, explicit_delete_races.rs, explicit_update_races.rs, implicit_delete_races.rs, implicit_update_races.rs, mod.rs` | 6 |
| `crates/shamir-engine/src/query/filter` | `README.md, caches, evaluation, mod.rs, tests` | 5 |
| `crates/shamir-engine/src/query/filter/caches` | `cond_cache.rs, field_path_cache.rs, mod.rs, query_ref_cache.rs` | 4 |
| `crates/shamir-engine/src/query/filter/evaluation` | `filter_node, mod.rs, planning, runtime, semantics` | 5 |
| `crates/shamir-engine/src/query/filter/evaluation/filter_node` | `membership.rs, mod.rs` | 2 |
| `crates/shamir-engine/src/query/filter/evaluation/planning` | `compile.rs, index_range.rs, mod.rs` | 3 |
| `crates/shamir-engine/src/query/filter/evaluation/runtime` | `core, mod.rs` | 2 |
| `crates/shamir-engine/src/query/filter/evaluation/runtime/core` | `eval.rs, eval_bytes.rs, eval_context.rs, mod.rs` | 4 |
| `crates/shamir-engine/src/query/filter/evaluation/semantics` | `fts.rs, mod.rs, numeric_cmp.rs, pattern_guard.rs, resolve.rs` | 5 |
| `crates/shamir-engine/src/query/filter/tests` | `caches, eval_tests, evaluation, mod.rs` | 4 |
| `crates/shamir-engine/src/query/filter/tests/caches` | `local_cond_cache_tests.rs, mod.rs, query_ref_cache_tests.rs` | 3 |
| `crates/shamir-engine/src/query/filter/tests/eval_tests` | `collections, comparisons, expressions, predicates, references, shared` | 6 |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/collections` | `binary_and_large_set_tests.rs, membership_literals_tests.rs, mod.rs, numeric_coercion_tests.rs, range_and_container_tests.rs` | 5 |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/comparisons` | `comparison_tests.rs, dec_cross_type_tests.rs, mod.rs, numeric_cmp_cross_path_tests.rs, resolve_compare_tests.rs, value_compare_null_tests.rs` | 6 |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/expressions` | `computed_tests.rs, cond_expr_tests.rs, cond_gap_tests.rs, mod.rs` | 4 |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/predicates` | `fts_tests.rs, logical_tests.rs, mod.rs, null_exists_tests.rs, string_regex_tests.rs, u64_big_filter_match_tests.rs` | 6 |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/references` | `field_ref_tests.rs, in_query_ref_column_tests.rs, mod.rs, query_ref_tests.rs` | 4 |
| `crates/shamir-engine/src/query/filter/tests/eval_tests/shared` | `helpers.rs, mod.rs` | 2 |
| `crates/shamir-engine/src/query/filter/tests/evaluation` | `cond_cache_tests.rs, eval_bytes_tests.rs, field_path_cache_tests.rs, filter_tests.rs, mod.rs` | 5 |
| `crates/shamir-engine/src/query/read` | `aggregation, docs, exec.rs, mod.rs, projection, tests` | 6 |
| `crates/shamir-engine/src/query/read/aggregation` | `accumulator.rs, function_inputs.rs, group_execution.rs, group_projection.rs, mod.rs, validation.rs` | 6 |
| `crates/shamir-engine/src/query/read/docs` | `README.md` | 1 |
| `crates/shamir-engine/src/query/read/projection` | `hashable_query_value.rs, mod.rs, order.rs, select_projection.rs` | 4 |
| `crates/shamir-engine/src/query/read/tests` | `aggregation, mod.rs, query_results` | 3 |
| `crates/shamir-engine/src/query/read/tests/aggregation` | `legacy_and_caps, mod.rs, numeric_and_functions` | 3 |
| `crates/shamir-engine/src/query/read/tests/aggregation/legacy_and_caps` | `aggregate_expression_execution_tests.rs, aggregate_expression_input_tests.rs, aggregate_null_input_tests.rs, hashable_query_value_tests.rs, mod.rs, string_agg_cap_tests.rs` | 6 |
| `crates/shamir-engine/src/query/read/tests/aggregation/numeric_and_functions` | `distinct_aggregate_tests.rs, function_aggregate_tests.rs, mod.rs, qv_aggregate_tests.rs, qv_exact_sum_tests.rs` | 5 |
| `crates/shamir-engine/src/query/read/tests/query_results` | `mod.rs, pagination_tests.rs, select_order_page, select_projection_tests.rs, select_refs_params_tests.rs` | 5 |
| `crates/shamir-engine/src/query/read/tests/query_results/select_order_page` | `grouping_tests.rs, mod.rs, ordering_tests.rs, pagination_distinct_tests.rs, qv_order_tests.rs, qv_select_tests.rs, select_tests.rs` | 7 |
| `crates/shamir-engine/src/query/write` | `README.md, mod.rs, tests` | 3 |
| `crates/shamir-engine/src/query/write/tests` | `mod.rs, write_op_tests.rs` | 2 |
| `crates/shamir-engine/src/validator` | `codecs, execution, mod.rs, registry, runtime, schema, tests` | 7 |
| `crates/shamir-engine/src/validator/codecs` | `decode.rs, encode.rs, mod.rs, native_adapter.rs, query_value_conv.rs` | 5 |
| `crates/shamir-engine/src/validator/execution` | `mod.rs, native_record_validator.rs, record_fields.rs, record_validator.rs, transform_spec.rs, validation_outcome.rs, wasm_record_validator.rs` | 7 |
| `crates/shamir-engine/src/validator/registry` | `mod.rs, persisted_validators.rs, persistence.rs, registry.rs, validator_binding.rs, validator_db.rs` | 6 |
| `crates/shamir-engine/src/validator/runtime` | `mod.rs` | 1 |
| `crates/shamir-engine/src/validator/schema` | `foreign_key.rs, mod.rs, rule_builder.rs, rules, schema_validator.rs, tests` | 6 |
| `crates/shamir-engine/src/validator/schema/rules` | `constraints.rs, cross_field.rs, field_rule.rs, format.rs, mod.rs, one_of_set.rs, type_tag.rs` | 7 |
| `crates/shamir-engine/src/validator/schema/tests` | `foreign_key_tests.rs, mod.rs, schema_rules` | 3 |
| `crates/shamir-engine/src/validator/schema/tests/schema_rules` | `builder_tests.rs, check_tests.rs, mod.rs, phase_b_tests.rs, schema_validator_tests.rs, unique_tests.rs` | 6 |
| `crates/shamir-engine/src/validator/tests` | `codecs, execution, mod.rs, registry` | 4 |
| `crates/shamir-engine/src/validator/tests/codecs` | `decode_tests.rs, encode_tests.rs, mod.rs, native_adapter_tests.rs, persistence_tests.rs, query_value_conv_tests.rs` | 6 |
| `crates/shamir-engine/src/validator/tests/execution` | `binding_tests.rs, mod.rs, record_fields_tests.rs, run_validators_tests.rs, validator_actor_tests.rs` | 5 |
| `crates/shamir-engine/src/validator/tests/registry` | `mod.rs, registry_atomic_tests.rs, registry_tests.rs, validator_db_tests.rs` | 4 |
| `crates/shamir-engine/tests` | `clean_horizon, crash_recovery, crash_recovery.rs, flush_persist_once_tests.rs, overlay_depth_probe.rs, prealloc_caps.rs, string_agg_caps.rs` | 7 |
| `crates/shamir-engine/tests/clean_horizon` | `admitted_field_id.rs, crash_matrix.rs` | 2 |
| `crates/shamir-engine/tests/crash_recovery` | `history_seed.rs, multi_table_drain.rs, vector_delta.rs, write_to_data.rs` | 4 |

## Manifest declaration plan

Each new `mod.rs` row above is a declarations/re-export-only file. Its child declarations are mechanically the direct `.rs` children in the projected destination tree, excluding itself; source file logical paths must be re-exported through the original facade where callers use old paths. Bench roots are an exception: their path manifest is a future explicit Cargo `[[bench]]` target path, never an ordinary module declaration.

## Privacy and logical module analysis

OBSERVED restricted declarations listed above are remapped only when their owning module moves. `pub(super)` exposes to the immediate parent; `pub(in crate::query::batch)` caps visibility at that ancestor, so module nesting changes the actual audience. In particular batch receipt helpers are bounded to the batch subtree; query-runner helpers and test helpers are parent-facing only. No `RepoInstance`/validator private data is moved by this Q slice. `validator/registry.rs:79` mutation hook is parent-only. Actual field accesses must be reviewed per moved implementation/test helper before approval; this proposal does not expand visibility.

OBSERVED mounts: only crash-recovery integration target at `tests/crash_recovery.rs:41,43`, relative to its target logical root; retain target root and child mount resolution. No query/validator module mount is observed. Global matcher comment hits are not mounts.

## Cargo roots and integration hooks

OBSERVED: `crates/shamir-engine/Cargo.toml:130–140,150–182,188–255` declares bench targets independently; bench source files remain target roots and must not be declared as ordinary `mod` children. Preserve each `[[bench]] name` and `required-features` (notably test-util-gated targets and fjall-specific targets) when any future target path changes. The tx_concurrent bench remains an independent target with explicit `[[bench]] path` set to the primary target file; extracted workload helpers are descendants under its target-specific `tx_concurrent_parts` module and must be loaded with target-relative paths. Keep `main` as target entry and preserve all Cargo target names/features.

Integration tests are separate Cargo targets under `tests/`; `crash_recovery.rs` has `#![cfg(feature="fjall")]` and path children. Keep target filename/path unless Cargo manifest target configuration is intentionally coordinated. Root-level F packing is out of scope and should occur after Q/T.

OBSERVED hooks within scope: query_runner test-only `FkRoleDecisionHook` and two `OnceLock` statics are defined at `query_runner.rs:257–300`, invoked at `:388–391,486–489`; consumers are `query/batch/tests/fk_reverse_cache_tests.rs:41,84–113`. Move shared hook definitions in A1 facade stage or keep as descendants / same logical owner; do not let Q extraction strand tests behind moved private scope. External consumer evidence found: `crates/shamir-db/src/shamir_db/tests/shutdown_quiesce_tests.rs:9` consumes the separate tx commit hook (not a Q hook), so unaffected but relevant downstream hook path audit.

## Regrouping and child declarations

DECIDED: semantic groups in destination mapping split batch execution API/runtime/FK/query owners; batch tests into executor/limits/referential-action/referential-race/query-semantics groups; filter evaluator/cache plus test families; read aggregation/projection and result/aggregate tests; validator schema rules and tests grouped by execution, registry and codecs. These preserve recognizable ownership rather than alphabetical bucketing. Every moved child requires parent `mod.rs` declaration and old facade re-export where prior logical paths are externally used. Generated new manifests are listed above.

## Risk and future-stage gates

Stage Q depends on A1 hook-facade decisions; do body moves only after stable facade selection. Bounded retry/SSI, FK plan/apply order, query transaction lifetime and result-size accounting require explicit behavior review. Aggregation splits must preserve one group walk and per-row accumulator interactions. Test splits must preserve all test functions, cfg/ignore attributes, helper access and compiled discovery. Crash recovery split remains integration-target-only. Q and table/T may work in parallel only with root manifests, Cargo target changes, test hooks and baseline changes serialized. Stage F owns final root packing; no root-level move here.

OBSERVED project checks: see JSON `splits`, `mappings`, `extra_files`, and `baseline`; no changes made. This is a planning artifact, not evidence that split files compile or that discovery is conserved.
