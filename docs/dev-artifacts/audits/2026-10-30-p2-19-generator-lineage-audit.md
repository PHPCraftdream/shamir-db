# P2-19 Round 2: generator lineage and consumer audit

Scope: `crates/` (including nested benches and examples), top-level `examples/`, `tests/`, `scripts/`, `docs/`, plus root `bench-iters.txt`. Top-level `benches/` does not exist. Searches included both canonical decimal constants, underscore/hex spellings, alternate common LCG constants (including 1664525/1013904223, 1103515245, 214013/2531011), `wrapping_mul`, high-word shifts 32/33/40, float divisors, `Lcg`, `next_f32`, `next_range`, `clustered_vectors`, revision, seed/fixture, recall and iteration-count references. Generic arithmetic/shift hits were classified by context, not treated as generators just because they shift or multiply. This is a static audit: no tests, benchmarks, builds or Cargo commands were executed. The orchestrator's gates are separate.

## Conclusions and action items

* `crates/shamir-index/src/vector/tests/quant/sq8_tests.rs`: the explicit canonical mirror was changed in the preceding chunk to the revision-2 high-24-bit mapping, f64 interpolation and clamped predecessor. It retains the raw-stream and exact Gaussian comparisons, and now compares uniform and valid/invalid range draws. Its lineage test is wired via `vector/mod.rs` (`cfg(test)`), `vector/tests/mod.rs`, `vector/tests/quant/mod.rs`; old high-32 arithmetic differs on the sampled uniform streams and opposite-extreme interpolation. The SQ8 tests' existing `0.98` recall gate and numerical tolerances are unchanged; CI must establish whether the revised dataset satisfies them. No tests were executed.
* The SQ8 local `clustered` documentation says it mirrors canonical generation for valid test inputs. This is true for its actual calls (positive dimension, `0 < k <= n`, positive sigma); it is not a general equivalence claim for all valid canonical inputs, since canonical clamps `k > n` and supports `n == 0`. No current call exercises that difference.
* `crates/shamir-engine/examples/persisted_hnsw.rs` was already updated to include `GENERATOR_REVISION` in emitted dataset provenance; other changed Rust files are the SQ8 fix and two corrected historical-fixture comments. No additional Rust mutation was performed while gates run.
* **Potential follow-up:** the historical `docs/dev-artifacts/benchmarks/vector/README.md` describes a five-field reproducibility key at line 23 and illustrates an unversioned report. Its sample is historical, not evidence for revision 2; a prospective note distinguishing the six-field key from historical examples would improve guidance without rewriting recorded results. `vector_bulk_compaction.rs`, `filtered_vector_search.rs`, and `sq8_hot_path.rs` use the canonical generator but do not themselves print its revision. Their benchmark outputs can be revision-sensitive; if standalone benchmark results are published without source revision, record the generator revision alongside them. This is a provenance recommendation, not evidence that an existing equality assertion fails.
* `generation_tests.rs` derives the supposed exact round-robin balance by nearest-centroid classification, rather than observing assignment; a generator change could move a point across a boundary. Retain the exact assertion; CI must validate it, not weaken it on speculation. No stored old-generator vector golden requiring replacement was identified in the inspected direct canonical consumers. A lack of a full-dataset byte golden means the revision pin plus same-seed equality alone does not independently pin every output byte.

## Canonical generator and tests

Paths below are repository-relative within this report.

| File | Check / disposition |
|---|---|
| `crates/shamir-bench-utils/src/vector_data.rs` | Source of truth: `clustered_vectors/2`, high 24 bits, one-step range in f64 and `hi.next_down()`, invalid bounds consume no state, polar Gaussian. No change in this audit. |
| `crates/shamir-bench-utils/src/vector_data/tests/lcg_tests.rs` | Predecessor-seed boundary vectors, adjacent signed/subnormal bounds, finite extremes and invalid-state checks independently exercise endpoints. Old high-32 conversion/old interpolation fail relevant cases. No golden update needed. |
| `crates/shamir-bench-utils/src/vector_data/tests/generation_tests.rs` | Revision string pinned; same-seed comparisons are reproducibility, not an independent fixed byte corpus; distinct seed, dimensions, clustering and balance checks are output-sensitive. Keep thresholds and exact counts; CI required. |
| `crates/shamir-bench-utils/src/vector_data/tests/gaussian_tests.rs` | Distribution and sigma-scaling statistical assertions use canonical generator; no literal byte fixtures; keep thresholds and validate in CI. |
| `crates/shamir-bench-utils/src/vector_data/tests/mod.rs` | Test manifest links generator, Gaussian and LCG tests; no change. |
| `crates/shamir-index/src/vector/tests/quant/sq8_tests.rs` | Explicit canonical lineage contract; migrated earlier. Raw state/constants, 64-draw float stream, ranges and exact Gaussian comparison are maintained; same-seed equality detects drift but does not replace canonical independent endpoint tests. |

## Canonical dataset consumers and benchmark provenance

| File | Check / disposition |
|---|---|
| `crates/shamir-engine/benches/vector_search/vector_search.rs` | Direct canonical generator for data and query; prints revision at registration. Seed, dimensions and counts stay fixed; query call with `n=1` clamps effective cluster count to one. No old byte golden. |
| `crates/shamir-engine/benches/vector_search/filtered_vector_search.rs` | Canonical data/query; separate LCG selects discrete allow-set indices via `>>33 % n` and is not the canonical float mapping. Existing counts and query parameters retained. Revision-output caveat above. |
| `crates/shamir-engine/benches/vector_search/vector_bulk_compaction.rs` | Canonical data; separately seeded integer LCG selects tombstones with `>>33 % n`. Raw recurrence compatibility is unaffected by float revision. Revision-output caveat above. |
| `crates/shamir-index/benches/sq8_hot_path.rs` | Canonical training data used to fit quantizer and drive hot-path timings; no fixed vector goldens or output threshold found. Revision-output caveat above. |
| `crates/shamir-engine/examples/vector_report.rs` | Canonical data and separately seeded queries; report emits revision and parameters. Recall is measured on newly generated data, not compared with a prior byte golden; old reports are not directly comparable as the same data. |
| `crates/shamir-engine/examples/persisted_hnsw.rs` | Canonical data, fresh dump/load and rebuild within same invocation; report now emits revision. No on-disk legacy golden is regenerated. |
| `crates/shamir-engine/benches/vector_search/quantization_f32_vs_sq8.rs` | Independent high-32 Gaussian clustered fixture intentionally matches graph-test historical dataset and reports paired f32/SQ8 recall; no canonical revision claim. Do not silently switch it or its historical comparison. |
| `bench-iters.txt` | Cached fixed iteration counts, not generated vector bytes. Entries include filtered vector (257–292), quantization (450–451), SQ8 hot path (523–532), bulk/compaction (687–697), vector search (698–709). Their calibrated timing usefulness can change with data or environment; do not hand-edit or infer a new count. Separate reviewed CI calibration if needed. |
| `scripts/bench/bench_gate.sh` | Uses committed iteration counts for comparable timing; no generator reproduction. No change. |
| `scripts/bench/bench_ab.py` | Plans/checks iteration digests, stages control counts and produces calibration artifacts. It does not pin dataset bytes; compare same-revision data or explicitly account for revision. No change. |
| `scripts/bench/bench_ab.toml` | Reviewed benchmark selection and counts; vector-search exclusion notes lack of reviewed cross-revision fixture/counts. No change. |
| `scripts/bench/bench-vector.sh` | Historical vector report/bench wrapper; launches consumers, no duplicate generator. Its Criterion-era presentation is outside this correction. No execution. |
| `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md` | Reviewed fixed control counts and separate calibration policy; no generator-byte assumption. No change. |

## Independent generator implementations and consumers

The shared recurrence alone is not a promise that every output mapping equals `clustered_vectors/2`. The following fixtures compare their own generated data against behavior/oracles, not a declared canonical dataset. Preserve them and their thresholds unless a separate migration is deliberately validated. Entries explicitly naming another fixture still match that fixture's old mapping.

| File | Check / no-change reason |
|---|---|
| `crates/shamir-index/src/vector/tests/quant/quantized_dist_tests.rs` | Historical high-32 clustered input; scalar-dequantized reference vs SIMD distance. Earlier comment corrected to say historical mapping; no canonical output pin. |
| `crates/shamir-index/src/vector/tests/quant/quantized_graph_tests/runner.rs` | Historical high-32 Gaussian data and seeded random vectors shared by graph tests; previous SQ8-mirror comment corrected. Keep fixture to retain graph recall history. |
| `crates/shamir-index/src/vector/tests/quant/quantized_graph_tests/fit_recall.rs` | Runner consumer; `0.95` SQ8-vs-f32 floor calibrated against stochastic HNSW layers and historical input, not revision-2 goldens. Preserve. |
| `crates/shamir-index/src/vector/tests/quant/quantized_graph_tests/fit_graph.rs` | Runner fixture drives fit and graph invariants; no canonical-byte claim. |
| `crates/shamir-index/src/vector/tests/quant/quantized_graph_tests/concurrent_fit.rs` | Runner fixture drives concurrent fit and seeded reconstruction; preserve same stream at both call sites. |
| `crates/shamir-index/src/vector/tests/quant/quantized_graph_tests/threshold_crossing.rs` | Own `>>33` per-seed vector fixture for threshold synchronization, not canonical clustered output. |
| `crates/shamir-index/src/vector/tests/quant/quantized_graph_tests/fit_failure_phases.rs` | Own `>>33` vector fixture for injected fit-failure paths; no revision contract. |
| `crates/shamir-index/src/vector/tests/quant/quantization_snapshot_tests.rs` | Own `>>33` single vectors and high-32 clustered generator mirroring historical graph fixture. Fresh snapshot round-trip/cold-start compares pre/post state, with `0.95` and `0.90` floors; not a frozen canonical byte corpus. |
| `crates/shamir-index/src/vector/tests/lifecycle/compaction_tests/quantized.rs` | High-32 clustered mirror explicitly points to graph generator, and uses `basic.rs` random vectors for other cases. `0.80` recall is an independent compaction smoke floor. |
| `crates/shamir-index/src/vector/tests/lifecycle/compaction_tests/basic.rs` | `>>33` deterministic vectors for compaction; shared with quantized compaction. |
| `crates/shamir-index/src/vector/tests/lifecycle/snapshot_tests.rs` | `>>33` fixture; persistence and CRC cases regenerate inputs in the same test, no revision-2 byte contract. |
| `crates/shamir-index/src/vector/tests/lifecycle/index_durability_tests.rs` | `>>33` fixture for index durability; no canonical mapping contract. |
| `crates/shamir-index/src/vector/tests/lifecycle/delta_log_tests.rs` | `>>33` fixture for delta logging; no canonical mapping contract. |
| `crates/shamir-index/src/vector/tests/lifecycle/crash_recovery_tests.rs` | `>>33` fixture for crash/restart; no canonical mapping contract. |
| `crates/shamir-index/src/vector/tests/lifecycle/cofilter_prefilter_tests.rs` | `>>33` contract-test-style vectors for filter comparisons; same raw lineage, different mapped output. |
| `crates/shamir-index/src/vector/tests/prune/delta_prune_tests.rs` | `>>33` seeded vectors for pruning; independent fixture. |
| `crates/shamir-index/src/vector/tests/hnsw/hnsw_rs_contract_tests.rs` | `>>33` vectors for upstream contract checks; canonical docs cite its multiplier, not identical float output. Preserve reference fixture. |
| `crates/shamir-index/src/vector/tests/hnsw/hnsw_adapter_tests/basic.rs` | `>>33` seeded HNSW adapter vectors, shared with `batch_ef.rs`; no canonical output contract. |
| `crates/shamir-index/src/vector/tests/hnsw/hnsw_adapter_tests/batch_ef.rs` | Consumes basic fixture for batching/ef invariants; preserve seed mapping. |
| `crates/shamir-index/src/vector/tests/hnsw/deadlock_regression_tests.rs` | Own `>>33` fixtures for race/deadlock scenarios; no canonical mapping contract. |
| `crates/shamir-index/src/vector/tests/backend/vector_restore_tests.rs` | Snapshot-style `>>33` vectors for restore tests; no canonical byte golden. |
| `crates/shamir-index/src/vector/tests/backend/simd_tests.rs` | Same raw LCG lineage but byte draws and separate high-32 float ranges for SIMD-vs-scalar comparisons. Both sides get identical generated inputs; no canonical float guarantee. |
| `crates/shamir-engine/src/table/tests/read/vector/filtered_ann/cases/selectivity.rs` | `>>33` divided by `u32::MAX` for independently seeded selectivity fixtures; tests filter behavior, not canonical data. |
| `crates/shamir-storage/src/key_bytes/tests/compare/eq_ord_tests.rs` | Same LCG recurrence, truncated byte stream for key equality/order checks; no vector dependency. |
| `crates/shamir-engine/src/db_instance/migration/tests/coordinator_tests.rs` | Same multiplier but increment 1 and integer high-word decisions for migration model; distinct algorithm and contract. |
| `crates/shamir-tx/src/tx/support/tests/staged_field_index_tests.rs` | 32-bit 1664525/1013904223 LCG selects operations and keys for a model test, not vectors. |
| `crates/shamir-tx/src/version/wal_event/tests/property_tests.rs` | Seeded xorshift/multiply property fixture, not LCG vector mapping. |
| `crates/shamir-engine/src/tx/tests/commit/publication/commit_tests.rs` | Seeded xorshift/multiply property fixture, not canonical vector mapping. |
| `crates/shamir-engine-query/src/query/read/tests/aggregation/numeric_and_functions/qv_aggregate_tests.rs` | Index-derived multiply/XOR permutation for aggregate inputs, not a mutable LCG. |
| `crates/shamir-engine/benches/query_reads/selection/streaming_topk.rs` | Index-derived LCG-style score permutation, not a generator consumer. |
| `crates/shamir-engine/benches/query_reads/ordering/read_path_matrix.rs` | Integer multiply/mask fixture names, not mutable LCG/float mapping. |
| `crates/shamir-engine/src/tx/tests/preparation/ssi_phantom_tests/helpers.rs` | `wrapping_mul` in hash calculation, not RNG. |
| `crates/shamir-storage/benches/storage_fjall_write_worker.rs` | Integer stride in key selection, not LCG. |

Other high-shift hits in WASM packed pointer/length handling, IP classification, and packed storage fields are unrelated to RNG. Generic `u32::MAX` and wrapping additions in protocol bounds, counters and storage buffers likewise do not consume this generator. Top-level `examples/`, `tests/`, and `scripts/` contain no direct canonical-symbol/LCG-constant hits; relevant script iteration-count consumers are listed above.

## Historical documents: retained, not rewritten

These are past prompts, measurements, review snapshots or checkpoints. Their old results and old mappings do not become revision-2 measurements merely because the generator changed. In particular `2026-07-05-quantization.md` records results for the independent graph-test-compatible generator, not canonical revision 2. Existing public/current guidance caveat for the vector README is recorded above.

* Vector benchmark history (`docs/dev-artifacts/benchmarks/vector/`): `README.md`, `2026-07-05-baseline.md`, `2026-07-05-bulk-compaction.md`, `2026-07-05-filtered-ann.md`, `2026-07-05-persisted-hnsw.md`, `2026-07-05-quantization.md`. Historical results and keys remain as recorded; unknown revision must not be relabeled.
* Cross-crate audit ledger (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/`): `SUMMARY.md`, `P0.md`, `P1.md`, `P2.md`, `P2-FIX-TASKS.md`, `P3.md`, `P4.md`. Old observations, findings, and assigned recipes are historical, not executable generators.
* Bench-utils topic audit (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-bench-utils/`): `SUMMARY.md`, `api-wire-protocol.md`, `concurrency-lockfree.md`, `correctness-tdd.md`, `error-handling-lifecycle.md`, `performance-hotpath.md`, `security-crypto.md`, `style-claude-md.md`. These document prior implementation/contracts; retain as audit evidence.
* Vector prompts (`docs/dev-artifacts/prompts/vector/`): `01-bench-infra.md`, `03-criterion-bench.md`, `04-vector-report.md`, `05-baseline-doc-hygiene.md`, `13-selectivity-bench.md`, `16-bulk-compaction-bench.md`, `17-sq8-quantizer.md`, `35-fit-window-empty-search.md`. They describe original intent/work, not current generated output.
* Remediation prompts (`docs/dev-artifacts/prompts/remediation/`): `01-migration-shadow-lsn-order.md`, `159-p0-bench-utils.md`, `220-p1-bench-utils.md`, `76-bench-ab-workflow.md`, `77-bench-fixtures.md`, `81-test-speed-vr5.md`, `p2/19-infra.md`. Retain committed instructions and review decisions.
* Calibration/benchmark prompts: `docs/dev-artifacts/prompts/ci/09-bench-ab-resolve.md`, `docs/dev-artifacts/prompts/ci/12-q5d-bench-cells.md`, `docs/dev-artifacts/prompts/ci/13-hot-path-ab-ratio.md`, `docs/dev-artifacts/prompts/oql-04/06-benchmarks-foreach.md`, `docs/dev-artifacts/prompts/post-alpha/110-f53d-ci-perf-gates-self-hosted.md`, `docs/dev-artifacts/prompts/post-alpha/136-f80-writer-drain-overhead-loom-ci.md`. Historical instructions about count generation, not fixture bytes.
* Research/perf: `docs/dev-artifacts/research/crate-extraction/01-core-utils.md`, `docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`, `docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`, `docs/dev-artifacts/research/2026-07-28-new-wave-readonly-review.md`, `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md`, `docs/dev-artifacts/perf/bench-sweep-allocator-comparison-2026-07-07.md`. Past extraction/review/iteration-count evidence; no rewrite.
* Checkpoints: `docs/checkpoints/2026-07-26-1215.md`, `docs/checkpoints/2026-07-27-0140.md`, `docs/checkpoints/2026-08-13-1145.md`, `docs/checkpoints/2026-09-02-2249.md`, `docs/checkpoints/2026-09-03-0809.md`, `docs/checkpoints/2026-10-10-1847.md`, `docs/checkpoints/2026-10-23-1516.md`, `docs/dev-artifacts/checkpoints/2026-07-07-bench-normalization-crush-fanout.md`, `docs/dev-artifacts/checkpoints/2026-07-07-bench-scale-tool-migration.md`, `docs/dev-artifacts/checkpoints/2026-07-09-audit-remediation-perf-radical.md`, `docs/dev-artifacts/checkpoints/2026-07-10-audit-remediation-500-inflight.md`, `docs/dev-artifacts/checkpoints/2026-07-10-audit-remediation-503-regrouped.md`, `docs/dev-artifacts/checkpoints/2026-07-10-audit-remediation-520-inflight.md`. Historical status/count references only.
