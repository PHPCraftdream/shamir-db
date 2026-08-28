# shamir-bench-utils -- API & wire-protocol design

## Summary

Tiny, dependency-free public surface: `vector_data` (deterministic clustered-dataset generator + `Lcg`) and the feature-gated `peak_mem` (global-allocator peak-RSS sampler). The crate builds no queries and no wire ops — there is no `serde`/`serde_json` anywhere in it — so the builder-only query-construction rule is trivially satisfied; the crate's de-facto "wire protocol" is the byte-identical determinism contract of `clustered_vectors`, and that is where the findings concentrate: the `peak_mem` "off by default" isolation claim does not survive how consumers actually enable it, the `ClusteredDataset` recoverability claim is false as written, and the cross-crate determinism lineage is versioned nowhere and enforced by nothing. Test coverage is reasonable for `vector_data` (inline `#[cfg(test)]`, deterministic-reproduction and degenerate cases covered); `peak_mem` has no tests at all.

## Findings

### 1. `peak_mem`'s global allocator is silently active for every bench/example/test binary of shamir-engine and shamir-index, contradicting the documented "off by default" contract
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:3-8` (contract claim), `crates/shamir-bench-utils/Cargo.toml:13-14`; enabled unconditionally at `crates/shamir-engine/Cargo.toml:107` and `crates/shamir-index/Cargo.toml:64`.
- Severity: medium
- Issue: The module doc promises the feature is "**off by default** so normal `cargo bench` paths are unaffected." In practice both consumers declare `shamir-bench-utils = { path = ..., features = ["peak_mem"] }` unconditionally in `[dev-dependencies]`, and with workspace resolver 2 (`Cargo.toml:3`) dev-dependency features are shared across *all* dev targets of the declaring package. So every bench, example, and test binary in shamir-engine and shamir-index links the `PeakAlloc` `#[global_allocator]` — including timing-only benches that merely use `vector_data` fixtures (e.g. `crates/shamir-engine/benches/filtered_vector_search.rs:81`) and never measure memory. Those benches pay `PeakAlloc`'s per-allocation atomic accounting in their *timing* numbers (an overhead `peak_mem.rs:37-38` itself acknowledges).
- Failure scenario: An `/opti` baseline-vs-after pair, or a workspace-wide bench sweep, mixes allocator-on and allocator-off cells; recorded ns/op carry a hidden allocator tax, and any consumer toggling the feature between runs invalidates every recorded baseline with no signal. This is precisely the comparability corruption the crate exists to prevent.
- Suggested fix: Remove `features = ["peak_mem"]` from both consumer manifests; instead add `peak_mem = ["shamir-bench-utils/peak_mem"]` to each consumer's own `[features]` and tag only the memory benches with `required-features = ["peak_mem"]` so the allocator is compiled in solely when opted in per invocation. Alternatively split the allocator into its own micro-crate so fixture-generation consumers can never unify it in. Correct the module doc to describe the real unification semantics either way.

### 2. `ClusteredDataset` doc claims "(k, σ) parameters are recoverable from the artefact alone" — σ never is, and k is clamped
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:108-111` (claim) vs `:113-118` (fields), `:179-183` (clamp); contradicts `:29-30` and `:156-159`.
- Severity: medium
- Issue: The struct carries only `vectors` and `centroids`. `sigma` and `seed` appear nowhere in the artefact, and `k()` returns `centroids.len()`, which is `k_eff = min(k_clusters, n)` — not the requested `k_clusters` (the `k_greater_than_n_clamps_silently` test at `:332-339` bakes in `ds.k() <= 3` for a request of `k = 10`). The module doc (`:29-30`) and fn doc (`:156-159`) simultaneously instruct callers to "surface" the full five-value key in reports — i.e. the values must be threaded externally — directly contradicting the struct doc's recoverability claim. The one report tool today works around it by defining its own `DatasetParams` struct (`crates/shamir-engine/examples/vector_report.rs:180-190`, used to print the header at `:336`), which is an admission that the artefact is *not* the single source of truth the doc promises.
- Failure scenario: A future tool trusts the struct doc, prints `ds.k()` and omits σ/seed: clamped-k reports get labelled with a k that never generated anything, and the printed "reproducibility key" cannot reproduce the data — the exact failure mode this crate was extracted to prevent.
- Suggested fix: Make the artefact actually self-describing: add request metadata to the struct (e.g. `pub params: DatasetParams { n, dim, k_clusters, sigma, seed }`, mirroring the consumer's own `DatasetParams`), or reword the doc to the truth (only `n`, `dim`, `k_eff` are recoverable; σ/seed/requested-k must be threaded by the caller).

### 3. The cross-crate LCG "lineage" contract is prose-only: ~13 hand-maintained mirrors, zero enforcement, and the stated justification for the duplication is stale
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:34-36` and `:40-44` (contract by comment); mirrors e.g. `crates/shamir-index/src/vector/tests/sq8_tests.rs:15-16,33,46-47`, `quantized_dist_tests.rs:36,49`, `quantized_graph_tests.rs:37,50`, `compaction_tests.rs:24,744`, `quantization_snapshot_tests.rs:48,88`, `hnsw_rs_contract_tests.rs:36,41`, `snapshot_tests.rs:40`, `vector_restore_tests.rs:80`, `delta_log_tests.rs:56`, `crash_recovery_tests.rs:98`, `cofilter_prefilter_tests.rs:36`, `hnsw_adapter_tests.rs:19`, `deadlock_regression_tests.rs:89`, plus `crates/shamir-engine/benches/vector_bulk_compaction.rs:96`.
- Severity: medium
- Issue: The crate's central export is a *determinism contract*: the constant, the high-32 extraction, the polar Box-Muller consumption order. The doc names one mirror (`hnsw_rs_contract_tests::lcg_vec`, which does exist) but at least 13 sites re-implement parts of it "mirroring `shamir_bench_utils`" by comment alone. No shared constant and no golden-stream test assert the mirrors still match. Worse, `sq8_tests.rs:15-16` justifies the duplication with "because `shamir-bench-utils` is not a dev-dependency of this crate" — false today: `crates/shamir-index/Cargo.toml:64` declares it as a dev-dependency, so the tests could consume `shamir_bench_utils::{Lcg, clustered_vectors}` directly.
- Failure scenario: A one-sided edit — a different Box-Muller variant, returning both variates, changing the bit-extraction width — silently breaks byte-identical lineage. Contract-test fixtures and bench data stop corresponding with no failing test, and cross-tool recall comparisons (the stated purpose, `vector_data.rs:3-7`) become apples-to-oranges.
- Suggested fix: Since the dev-dependency now exists, delete the mirrors in shamir-index tests and use `shamir_bench_utils::{Lcg, clustered_vectors}`; where a copy must remain, pin it with a golden test asserting the first N stream values documented in one place; expose `pub const LCG_MULT`/`LCG_INC` so the constant has a single canonical address.

### 4. Reproducibility key excludes any generator/dataset-format version
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:21-30`, `:156-159`.
- Severity: low
- Issue: The documented key is `(n, dim, k_clusters, sigma, seed)`, but the byte stream is as much a function of the *code version* as of the key: generation order, Box-Muller variant, and centroid-draw order all feed it. Nothing in the public API identifies the algorithm, and consumers print only the key in report headers (`vector_report.rs:336`).
- Failure scenario: Any generation-affecting change silently produces different data under the same key; every previously recorded comparison keyed on the five-tuple becomes unreproducible with no warning.
- Suggested fix: Add `pub const DATASET_FORMAT_VERSION: u32` (bumped on any generation-affecting change), carry it on `ClusteredDataset` (see finding 2), and print it in report headers next to the tuple.

### 5. `peak_mem` module docs still teach the removed Criterion API
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:10-30`, `:43-44`.
- Severity: low
- Issue: "Usage with Criterion `iter_custom`", "before `criterion_main!`", and the report-bytes-as-`Duration::from_nanos` trick predate the 2026-07-07 workspace migration off Criterion (lib.rs:9-12 documents the tier-API removal; CLAUDE.md explicitly warns bench authors not to reach for Criterion APIs). The examples are `rust,ignore` and `doctest = false`, so nothing compile-checks them — a new bench author copying the module's own docs is steered straight at APIs that no longer exist in this repo.
- Failure scenario: Copied verbatim into a new bench, the example fails to compile and nudges the author toward reinstating Criterion patterns the workspace removed.
- Suggested fix: Rewrite the examples against `bench_scale_tool::Harness` (template: `crates/shamir-engine/benches/tx_pipeline.rs`) or against the real usage pattern already in-tree (`crates/shamir-index/benches/create_index_streaming.rs:179-197`: `setup()` once, `reset()` before, `current_peak()` after).

### 6. `clustered_vectors` panics on `k_clusters == 0` / `dim == 0` instead of returning `Result`
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:171-172`.
- Severity: low
- Issue: CLAUDE.md's error-handling rule prefers `Result<T, E>` with `panic!` reserved for programmer-bug invariants. The asserts are documented under `# Panics` and every current call site passes compile-time constants, so this sits inside the carve-out — but the parameters are runtime-shaped values, and the report tooling is drifting toward parameterised runs (`vector_report.rs` threads them through a `DatasetParams` struct).
- Failure scenario: A future CLI/env-driven caller passes unsanitised `k_clusters == 0` and gets a panic mid-report instead of a validation error.
- Suggested fix: Borderline as-is; if/when parameters become externally supplied, validate at that boundary and return `Result` (or keep the asserts and require callers to validate). Document explicitly that the asserts assume compile-time constants.

### 7. Cargo.toml `description` advertises the removed BENCH_QUICK tier feature
- File:line: `crates/shamir-bench-utils/Cargo.toml:6` vs `crates/shamir-bench-utils/src/lib.rs:9-12`.
- Severity: low
- Issue: The package description still reads "BENCH_QUICK env-var support for /opti baseline/after pairs" — that is the removed Criterion-era API. The crate's actual surface today is vector fixtures + optional peak-RSS sampling. Metadata drift on the crate's public face (`publish = false`, but it is still the first thing a reader sees in the manifest).
- Failure scenario: A contributor greps manifests for "BENCH_QUICK", concludes the env-var support lives here, and wires against a feature that no longer exists.
- Suggested fix: Update the description to match the crate (e.g. "Shared bench helpers: deterministic clustered vector datasets and optional peak-RSS sampling").

### 8. `Lcg::next_f32` can return exactly 1.0, violating its documented `[0, 1)` range
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:71-76`.
- Severity: nit
- Issue: `(high as f32)` rounds `u32::MAX` (4294967295) up to 4294967296.0 in f32, so the quotient is exactly 1.0 with probability ≈ 2⁻³² per draw, while the doc says "Uniform `f32` in `[0, 1)`". Downstream impact is nil today (Box-Muller's `s < 1.0` check rejects the degenerate draw; centroid bounds shift by 2⁻³²), but the documented contract is false.
- Failure scenario: Only theoretical; any future consumer relying on `x < 1.0` (e.g. indexing into a length-N slice after scaling) has a one-in-four-billion off-by-one.
- Suggested fix: Consume 24 bits, which are exactly representable: `((self.next_u64() >> 40) as f32) / (1u32 << 24) as f32` — note this changes the stream mapping, so pair it with the version bump from finding 4.

### 9. `peak_mem::measure`/`measure_async` have an undocumented process-global single-flight contract; `setup()`'s stated rationale is dubious; tuple return
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:48-52`, `:57-110`.
- Severity: nit
- Issue: `reset()` clobbers one process-wide counter, so two concurrent `measure` calls (or any concurrent `current_peak()`) corrupt each other's readings — acceptable for today's sequential bench cells but nowhere stated (the `measure_async` doc only warns about interleaved allocations, `:98-101`). `setup()`'s justification — "so the linker doesn't strip the global allocator in LTO builds" (`:49-50`) — is not how `#[global_allocator]` registration works (it is applied whenever the crate is linked, referenced or not); the function is an honest no-op but the doc mis-explains it. `measure` returns an anonymous `(R, usize)` tuple; a named struct would be self-documenting at call sites.
- Failure scenario: A future parallel bench harness measures two workloads concurrently and gets garbage peaks; readers of `setup()` inherit a cargo-cult linker superstition.
- Suggested fix: Document "at most one measurement in flight per process" on `reset`/`current_peak`/`measure`; simplify `setup()` to a plain documentation anchor without the linker claim (or remove it); consider `struct Measurement<R> { result: R, peak_bytes: usize }`.
