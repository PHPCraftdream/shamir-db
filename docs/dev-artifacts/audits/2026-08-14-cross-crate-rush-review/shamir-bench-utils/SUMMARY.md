<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-bench-utils — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The retained open mechanisms are reproducible from source, but several Medium labels describe local-tooling or prospective coverage concerns rather than demonstrated runtime damage. No plan completion is source-proven. Dependency reset internals are available and verified; cancellation inheritance and mandatory export splitting remain refuted.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 17 | 15 | 0 | 0 | 1 | 1 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — `peak_mem` has zero test coverage; its load-bearing measurement contract rests on unpinned `peak_alloc` 0.3 semantics

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No registered helper tests exercise peak_mem; ignored doctests are disabled. The version is nevertheless checksummed and locked. A discriminating reset oracle must first create and release a higher watermark, then verify reset-to-current under isolated allocator activity; a bare peak&gt;=allocation-size assertion can accept a stale peak.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L57); [crates/shamir-bench-utils/Cargo.toml:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/Cargo.toml#L11); [Cargo.lock:2396](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2396).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — Box-Muller scale is unpinned: a transcription bug in `next_gaussian` would pass the entire suite

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The formula includes the required division by s. Repeatability compares identical implementations, while clustering only bounds noise from above. Missing independent scale or moment coverage is confirmed; the specific complete-suite mutation outcome is not.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L96); [crates/shamir-bench-utils/src/vector_data.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L234); [crates/shamir-bench-utils/src/vector_data.rs:326](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L326).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — `Lcg::next_f32` violates its documented `[0, 1)` contract — can return exactly 1.0

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The top 128 high words round upward before division. Odd-multiplier invertibility supplies a predecessor seed for an endpoint successor state. Gaussian rejection excludes that disk endpoint, but does not repair the public uniform contract.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L67); [crates/shamir-bench-utils/src/vector_data.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L75); [crates/shamir-bench-utils/src/vector_data.rs:95](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L95).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — `round_robin_balances_clusters` asserts a statistical property as an exact equality

Status: `unverified`. Current risk: `nit`.

Prior-cycle decision: `unverified`.

The oracle observes geometric assignment rather than i%k, and equal counts do not prove each point retained its construction label. Its fixed seed and small noise could still have ample separation; no affected target or numerical margin establishes a portability failure.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:201](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L201); [crates/shamir-bench-utils/src/vector_data.rs:271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L271); [crates/shamir-bench-utils/src/vector_data.rs:285](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L285).

Grouping/duplicate: [correctness-tdd.md#9](correctness-tdd.md#review-9). This is not an additional independent defect.

<a id="review-2-1"></a>

### Claim 2.1 — `peak_mem`'s measurement window is unowned: concurrent or aborted use silently corrupts the one process-global peak watermark

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

An outer measurement can allocate and free its maximum before an inner measurement resets it. Separately, published peak_alloc 0.3.0 src/lib.rs:108–109 and :125–127 permit reset-load, allocation-update, stale-reset-store, leaving peak below live allocation. Source: https://docs.rs/crate/peak_alloc/0.3.0/source/src/lib.rs. Subsequent-helper cancellation inheritance is refuted; current raw samples are sequential.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L89); [crates/shamir-bench-utils/src/peak_mem.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L106); [crates/shamir-engine/benches/streaming_topk.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/streaming_topk.rs#L143); [Cargo.lock:2396](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2396).

<a id="review-3-1"></a>

### Claim 3.1 — feature-gated `#[global_allocator]` in a library crate is a process-wide side effect

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The allocator declaration remains in the library and applies beyond measurement calls in linked consumers. Current edges are development-only; no ordinary database-production or secret-handling path is established.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L39); [crates/shamir-engine/Cargo.toml:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L107); [crates/shamir-index/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L64).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — Fixture generator allocates one heap `Vec` per point (allocation-in-loop: n+1 allocations per call)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each positive-dimensional row has its own allocation, with separate centroid storage and consumer ingestion clones. This is structural setup-cost debt under the project convention, not proof of measured fragmentation, latency loss, or false RSS accounting.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:188](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L188); [crates/shamir-bench-utils/src/vector_data.rs:202](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L202); [crates/shamir-engine/benches/vector_search.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/vector_search.rs#L125).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — `peak_mem`'s allocator is silently active for every bench/example/test binary of shamir-engine and shamir-index, contradicting the documented "off by default" contract

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Both development edges enable peak_mem without invocation opt-in. Fixture-only benches actually import the helper and inherit its allocator without setup. Unused dependency declarations alone do not establish every test binary's linkage.

Evidence: [crates/shamir-engine/Cargo.toml:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L107); [crates/shamir-index/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L64); [crates/shamir-engine/benches/filtered_vector_search.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/filtered_vector_search.rs#L25); [crates/shamir-bench-utils/src/peak_mem.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L39).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — `ClusteredDataset` doc claims "(k, σ) parameters are recoverable from the artefact alone" — σ never is, and k is clamped

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

n=0 provides a direct sigma-recovery counterexample: changing sigma leaves the artifact unchanged. Requested k is also lost when positive n clamps it. Effective k is legitimate generation metadata, however; substituting effective k can reproduce the same data with other inputs retained.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L111); [crates/shamir-bench-utils/src/vector_data.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L113); [crates/shamir-bench-utils/src/vector_data.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L179); [crates/shamir-engine/examples/vector_report.rs:184](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L184).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — The cross-crate LCG "lineage" contract is prose-only: ~13 hand-maintained mirrors, zero enforcement, and the stated justification for the duplication is stale

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Named mirror modules are registered and duplicate the same integer constants. No canonical comparison exists, and sq8's dependency justification is false. Different uniform mappings and deletion selection are intentional variants; current cross-tool corruption is not proven.

Evidence: [crates/shamir-index/src/vector/tests/sq8_tests.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/tests/sq8_tests.rs#L16); [crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs#L43); [crates/shamir-index/src/vector/tests/mod.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/tests/mod.rs#L8); [crates/shamir-engine/benches/vector_bulk_compaction.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/vector_bulk_compaction.rs#L98); [crates/shamir-index/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L64).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Reproducibility key excludes any generator/dataset-format version

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The artifact and generated header provide neither algorithm version nor source revision. This is a report-provenance gap relative to the release checklist, not proof that fixed-code reproducibility is broken. Recording a source SHA can satisfy provenance without mandating a new public format constant.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L158); [crates/shamir-engine/examples/vector_report.rs:332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L332); [docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md:245](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md#L245).

Grouping/duplicate: [api-wire-protocol.md#4](api-wire-protocol.md#review-4). This is not an additional independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — Cargo.toml `description` advertises the removed BENCH_QUICK tier feature

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The package metadata still advertises tier functionality absent from the declaration-only current public surface. This is documentation drift, not an active environment-processing defect.

Evidence: [crates/shamir-bench-utils/Cargo.toml:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/Cargo.toml#L6); [crates/shamir-bench-utils/src/lib.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/lib.rs#L9).

Grouping/duplicate: [api-wire-protocol.md#7](api-wire-protocol.md#review-7). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — `clustered_vectors` validates arguments with `assert!` instead of `Result`/`thiserror`, one caller feeds it externally-controlled input, and the second panic path (`dim == 0`) is undocumented and untested

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

VR_K_CLUSTERS=0 reaches the cluster assert. Independently, PH_DIM=0 in persisted_hnsw reaches the dimension assert; vector_report itself filters zero dimensions. These are operator-controlled local examples, not remote database input. The dim panic and sigma domain lack documentation and dedicated coverage.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L163); [crates/shamir-bench-utils/src/vector_data.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L171); [crates/shamir-engine/examples/vector_report.rs:406](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L406); [crates/shamir-engine/examples/persisted_hnsw.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/persisted_hnsw.rs#L207); [crates/shamir-engine/examples/persisted_hnsw.rs:363](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/persisted_hnsw.rs#L363).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — Entire test module embedded inline in `vector_data.rs`, violating the mandatory `tests/` layout

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The complete test block remains inline despite the explicit layout prohibition. Its registration is intact; missing discovery and a runtime High do not follow from placement.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L217); [crates/shamir-bench-utils/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/lib.rs#L17); [CLAUDE.md:594](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L594).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — `peak_mem.rs` docs still teach Criterion as the module's usage pattern — the harness the workspace removed and banned — and `vector_data.rs` still cites "the criterion bench"

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Criterion recipes and qualifiers remain while registered consumers use Harness. Disabled, ignored examples provide no compilation oracle; replacing them should describe one-shot heap sampling separately from latency timing.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L10); [crates/shamir-bench-utils/src/peak_mem.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L44); [crates/shamir-bench-utils/src/vector_data.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L3); [crates/shamir-engine/benches/vector_search.rs:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/vector_search.rs#L53).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — `vector_data.rs` carries three public exports (`Lcg`, `ClusteredDataset`, `clustered_vectors`) — borderline against one-file-one-export

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The rule allows closely coupled groups. The generator directly owns RNG consumption and returns the dataset type. A file split is an organizational option, not mandatory repair.

Evidence: [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505); [crates/shamir-bench-utils/src/vector_data.rs:185](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L185); [crates/shamir-bench-utils/src/vector_data.rs:209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L209).

Grouping/duplicate: [style-claude-md.md#4](style-claude-md.md#review-4). This is not an additional independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — Reproducibility key described inconsistently: "(k, σ, seed) triple" vs. five values

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The module names only k, sigma, and seed, but n and dim affect output. The function correctly lists five inputs; the downstream bench incorrectly calls its five-value tuple a triple.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L29); [crates/shamir-bench-utils/src/vector_data.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L158); [crates/shamir-engine/benches/vector_search.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/vector_search.rs#L19).

Grouping/duplicate: [style-claude-md.md#5](style-claude-md.md#review-5). This is not an additional independent defect.

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 9 | 0 | 0 | 0 | 0 | 0 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Neither consumer passthrough feature nor memory-target gate exists. Such gates can provide invocation opt-in, not isolation between targets sharing an invocation. Moving allocator declaration into a bench must retain access to the same counters; document activation, actual linkage, and allocator configuration.

Evidence: [crates/shamir-engine/Cargo.toml:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L107); [crates/shamir-engine/Cargo.toml:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L195); [crates/shamir-index/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L64); [crates/shamir-index/Cargo.toml:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L87); [crates/shamir-bench-utils/src/peak_mem.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L39).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Independent Gaussian/stream oracles and revision metadata remain absent. Exact transcendental goldens require target qualification. Canonicalizing every helper is unsafe without preserving distinct float mappings, deletion selection, and clamp boundaries. Imports must use vector_data; public constants and a dedicated format version are options, not independently mandatory fixes.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:42](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L42); [crates/shamir-bench-utils/src/vector_data.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L234); [crates/shamir-bench-utils/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/lib.rs#L17); [crates/shamir-index/src/vector/tests/sq8_tests.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/tests/sq8_tests.rs#L63); [crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs#L43).

<a id="plan-p1-3"></a>

### Plan P1.3 — P1.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Rows remain nested and metadata absent. A flat rewrite must preserve draw order, use checked dimension products, retain n=0 centroid behavior, and update public-field callers. Adapter batches still require owned Vec rows, so flattening alone does not remove ingestion allocations. Compare historical output, not two calls to the new implementation.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L113); [crates/shamir-bench-utils/src/vector_data.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L179); [crates/shamir-bench-utils/src/vector_data.rs:202](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L202); [crates/shamir-index/src/vector/adapter.rs:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/adapter.rs#L107); [crates/shamir-engine/benches/vector_search.rs:121](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/vector_search.rs#L121).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No ownership guard or measurement tests exist. A cooperating single-flight guard addresses overlapping windows, not arbitrary allocation or the dependency load/store race. If introduced, its claim must release on unwind/cancellation without restoring another window's peak. Reset-to-zero tests and current_thread-only isolation are invalid recipes.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L57); [crates/shamir-bench-utils/src/peak_mem.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L89); [crates/shamir-bench-utils/src/peak_mem.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L98); [crates/shamir-bench-utils/src/peak_mem.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L106); [Cargo.lock:2396](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2396).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Tests are still inline. Directory migration is valid policy remediation provided module registration survives. It need not split the coupled implementation, and cannot by itself add behavioral coverage or make @vector select this package.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L217); [crates/shamir-bench-utils/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/lib.rs#L17); [scripts/test.sh:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/test.sh#L107); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505); [CLAUDE.md:594](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L594).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Criterion documentation is unchanged. A replacement should show real one-shot reset/workload/read heap sampling alongside Harness timing, without encoding bytes as a Duration or promising per-task isolation. Deleting unused public helpers is not necessary merely to fix the examples.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L10); [crates/shamir-bench-utils/src/peak_mem.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L28); [crates/shamir-engine/benches/streaming_topk.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/streaming_topk.rs#L113); [crates/shamir-index/benches/create_index_streaming.rs:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/benches/create_index_streaming.rs#L177).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Neither documented-assert nor Result-based resolution is complete. Validate both local environment callers and define sigma expectations. Replacing asserts with Result followed by expect at user-controlled boundaries merely relocates the panic; expect is appropriate only for trusted fixed fixtures. Preserve explicit zero-n and clamp behavior.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L171); [crates/shamir-bench-utils/src/vector_data.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L179); [crates/shamir-engine/examples/vector_report.rs:406](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L406); [crates/shamir-engine/examples/persisted_hnsw.rs:363](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/persisted_hnsw.rs#L363).

<a id="plan-p2-8"></a>

### Plan P2.8 — P2.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The endpoint conversion and missing boundary oracle remain. A 24-bit map establishes next_f32&lt;1 but changes fixture bytes and requires an explicit compatibility/provenance decision. It does not establish next_range&lt;hi for arbitrary bounds; add a separate adjacent-float interpolation oracle or narrow that contract.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L75); [crates/shamir-bench-utils/src/vector_data.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L81); [crates/shamir-bench-utils/src/vector_data.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L158).

<a id="plan-p2-9"></a>

### Plan P2.9 — P2.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Description and five-input wording remain stale. Do not weaken exact balance assertions without an established failure or replace construction-balance coverage with tolerance alone. Mandatory Lcg extraction is refuted; keeping the coupled unit is allowed. These optional facets do not make the remaining metadata work complete.

Evidence: [crates/shamir-bench-utils/Cargo.toml:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/Cargo.toml#L6); [crates/shamir-bench-utils/src/vector_data.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L29); [crates/shamir-bench-utils/src/vector_data.rs:285](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L285); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — The tracking allocator always moves reallocations, changing benchmark work rather than merely adding counters

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

Checksummed published peak_alloc 0.3.0 src/lib.rs:168–185 implements realloc as System.alloc(new_size), accounting addition, copy of min(old,new), System.dealloc(old), and accounting subtraction; it never delegates System.realloc. A successful resizing request therefore incurs a mandatory copy and temporary old-plus-new allocation watermark. Linked fixture-only timing benches inherit this implementation. This is a source-proven instrumentation behavior omitted from assigned claims, not measured slowdown or memory unsafety. Source: https://docs.rs/crate/peak_alloc/0.3.0/source/src/lib.rs.

Evidence: [Cargo.lock:2396](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2396); [crates/shamir-bench-utils/src/peak_mem.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L39); [crates/shamir-bench-utils/src/peak_mem.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L37); [crates/shamir-engine/Cargo.toml:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L107); [crates/shamir-index/benches/create_index_streaming.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/benches/create_index_streaming.rs#L24); [crates/shamir-engine/benches/vector_search.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/vector_search.rs#L54).

<a id="observation-new-2"></a>

### Observation NEW.2 — next_range can reach hi even when next_f32 is strictly inside its promised interval

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

Choose valid finite bounds lo=1, hi=1+2^-23 and a reachable uniform value 0.75. The interpolation computes 1+3*2^-25, which rounds to hi. The LCG's invertible step permits a predecessor seed whose successor high word is 0xC0000000, giving exactly 0.75 under both present and proposed 24-bit mappings. This is independent of the assigned next_f32=1 endpoint mechanism. Current fixture callers use different bounds, so impact is a public helper contract and incomplete fix recipe, not demonstrated production failure.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L67); [crates/shamir-bench-utils/src/vector_data.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L75); [crates/shamir-bench-utils/src/vector_data.rs:78](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L78); [crates/shamir-bench-utils/src/vector_data.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L81); [crates/shamir-bench-utils/src/vector_data.rs:214](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L214).

## Evidence and recipe corrections

- The overview and correction bullet saying dependency-reset implementation evidence is unavailable contradict the parent refinement and independently inspected checksummed peak_alloc 0.3.0 archive. Remove the obsolete limitation.
- Neither no production hot path nor current_thread proves the allocator path is harmless: linked timing workloads execute it, and exact realloc behavior is more than atomic accounting.
- Requested-k loss does not imply effective-k reports cannot reproduce positive-n data. min(requested_k,n) is the only k used for draws; sigma and seed still require provenance.
- Published peak figures are tracked allocation-layout watermarks, not RSS. The RSS caller takes two current samples, not a continuously observed build high-water mark.
- The original numerical mutation-passes-suite, fragmentation, ratio-neutrality, and cross-target-flake assurances remain unsupported by execution evidence.
- There are 53 current claim rows across these eight documents, nine current plan rows, and no current observations; repeated historical bodies are not additional current decisions.

## Module scope and limitations

Coverage: 8 assigned documents, 53 current claim rows, 9 plan rows, 0 pre-existing observation rows; 2 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [SUMMARY.md](SUMMARY.md); [api-wire-protocol.md](api-wire-protocol.md); [concurrency-lockfree.md](concurrency-lockfree.md); [correctness-tdd.md](correctness-tdd.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [performance-hotpath.md](performance-hotpath.md); [security-crypto.md](security-crypto.md); [style-claude-md.md](style-claude-md.md).

- Read-only inspection only; no files changed and no builds, tests, benchmarks, reproductions, scripts, or child agents executed.
- HEAD matched the required base. Changed lockfile and toolchain metadata were read through git show at that base; unrelated checkout edits were untouched.
- All eight assigned documents, including historical bodies and recipes, were read. No current Observation rows or assigned TASK_GROUPS.md exist.
- No measured allocator overhead, latency, fragmentation, RSS impact, or cross-target balance-test failure is established.
- Actual linkage of every unused development dependency into every development binary was not established.
- This is bounded claim revalidation, not exhaustive dependency memory-safety or whole-workspace certification.

## Guarantee checks

- **The review uses frozen dependency and toolchain identities.** — `supported`. Frozen metadata identifies peak_alloc 0.3.0, bench-scale-tool 0.1.0, memory-stats 1.2.0, and Rust 1.94.0. The published peak_alloc archive was inspected to stdout and its SHA256 matched ccc90935a8dd139fdf341762773687a1e361d3f54b396a55d9dd1f7001e484bb. Relevant published harness and memory-stats archive source was also inspected. Reference: Cargo.lock:2396; Cargo.lock:404; Cargo.lock:2091; rust-toolchain.toml:15.
- **Fixed inputs produce reproducible fixtures on a given target.** — `supported`. State is local, arithmetic wraps explicitly, and centroid/noise draw order is fixed. This supports same-implementation reproducibility, not identity across algorithm revisions or targets. The registered equality test is neither a historical golden nor a bitwise comparison. Reference: crates/shamir-bench-utils/src/vector_data.rs:23; crates/shamir-bench-utils/src/vector_data.rs:158; crates/shamir-bench-utils/src/vector_data.rs:185.
- **The Gaussian primitive uses the intended polar Box-Muller formula.** — `supported`. Acceptance requires 0 &lt; s &lt; 1 and the multiplier is sqrt(-2*ln(s)/s). Current formula correctness does not establish measured distribution quality; no independent scale oracle is registered. Reference: crates/shamir-bench-utils/src/vector_data.rs:89; docs/dev-artifacts/prompts/vector/01-bench-infra.md:38.
- **Lcg::next_f32 returns values strictly below one.** — `diverges`. High words 4294967168 through 4294967295 round to 2^32 before division. The odd LCG multiplier makes such successor states reachable from public seeds. Integer-to-float rounding follows the [Rust numeric-cast rules](https://doc.rust-lang.org/reference/expressions/operator-expr.html#numeric-cast). Reference: crates/shamir-bench-utils/src/vector_data.rs:71.
- **A 24-bit uniform conversion would also establish next_range's general half-open interval guarantee.** — `diverges`. Even with uniform 0.75, lo=1 and hi=1+2^-23 produce an exact intermediate 1+3*2^-25 that rounds to hi. Fixing next_f32 alone therefore does not establish the unrestricted next_range contract. Reference: crates/shamir-bench-utils/src/vector_data.rs:78; crates/shamir-bench-utils/src/vector_data.rs:81.
- **ClusteredDataset preserves recoverable generation parameters.** — `diverges`. Only vectors and centroids are stored. With n=0, arbitrary sigma values produce identical artifacts. Positive-n clamping loses requested k, although effective k remains recoverable and can reproduce the same generated bytes when other inputs are retained. Reference: crates/shamir-bench-utils/src/vector_data.rs:109; crates/shamir-bench-utils/src/vector_data.rs:113; crates/shamir-bench-utils/src/vector_data.rs:179.
- **A point's nearest centroid necessarily identifies its generating cluster.** — `diverges`. The construction label is i modulo effective k. Noise is not constrained to that centroid's Voronoi region; nearest-centroid assignment is a proxy, not a guaranteed inverse. The report correctly uses BruteForceAdapter rather than this proxy for exact neighbors. Reference: crates/shamir-bench-utils/src/vector_data.rs:110; crates/shamir-bench-utils/src/vector_data.rs:201; crates/shamir-bench-utils/src/vector_data.rs:204.
- **reset sets peak to current tracked allocation without losing concurrent activity.** — `diverges`. Published peak_alloc 0.3.0, src/lib.rs:108–109, loads CURRENT then stores PEAK. An intervening allocation can update both counters and have its peak overwritten by the stale store. Exact source: https://docs.rs/crate/peak_alloc/0.3.0/source/src/lib.rs. Reference: crates/shamir-bench-utils/src/peak_mem.rs:54; Cargo.lock:2396.
- **Cancellation without a drop guard makes the next helper inherit the aborted window's historical peak.** — `diverges`. Both helpers reset before the next workload. Surviving allocations legitimately belong to its process baseline; continuing foreign work is contamination, not historical-peak inheritance. Reference: crates/shamir-bench-utils/src/peak_mem.rs:89; crates/shamir-bench-utils/src/peak_mem.rs:106.
- **current_thread provides accurate per-task peak isolation.** — `diverges`. Counters are process-global. Other tasks on that same runtime, other threads, blocking workers, and nested resets remain observable. Sequential helper invocation does not prove allocator quiescence. Reference: crates/shamir-bench-utils/src/peak_mem.rs:98; crates/shamir-bench-utils/src/peak_mem.rs:40.
- **peak_mem samples peak RSS with one atomic add per allocation.** — `diverges`. Published peak_alloc 0.3.0 tracks successful allocation layout sizes, not resident pages. add_memory performs fetch_add plus fetch_max; realloc explicitly allocates, copies, and frees. Exact source: https://docs.rs/crate/peak_alloc/0.3.0/source/src/lib.rs. Reference: crates/shamir-bench-utils/src/lib.rs:6; crates/shamir-bench-utils/src/peak_mem.rs:37; Cargo.lock:2396.
- **setup activates allocator tracking and ordinary fixture consumers remain unaffected.** — `diverges`. setup only references the static. Both development dependencies enable its feature, including actual fixture-only consumers. A linked dependency may supply the allocator under the [global_allocator rules](https://doc.rust-lang.org/reference/runtime.html#the-global_allocator-attribute); activation is not a runtime setup operation. Reference: crates/shamir-bench-utils/src/peak_mem.rs:34; crates/shamir-bench-utils/src/peak_mem.rs:48; crates/shamir-engine/Cargo.toml:107; crates/shamir-index/Cargo.toml:64.
- **Nine helper unit tests are registered and selected by @vector.** — `diverges`. All nine are registered for the helper's library test target, without additional feature or platform gating. @vector selects index and engine, not this package. Workspace library selection or explicit helper-package selection can include them; doctest=false does not disable unit tests. Reference: crates/shamir-bench-utils/src/lib.rs:17; crates/shamir-bench-utils/src/vector_data.rs:217; scripts/test.sh:107; scripts/test.sh:177.
- **Source contains no own locks, unsafe code, secret handling, or wire/query construction.** — `supported`. The three source files have no such operations. Lcg is explicitly non-cryptographic and current dependency edges are development-only. This is scoped source evidence, not a guarantee about all dependency internals or future consumers. Reference: crates/shamir-bench-utils/src/lib.rs:14; crates/shamir-bench-utils/src/vector_data.rs:52; crates/shamir-bench-utils/src/peak_mem.rs:32.
- **The report's RSS field is distinct from peak_mem and represents an observed process-memory sample.** — `supported`. It reads memory-stats physical_mem and takes the maximum of pre/post samples. Published memory-stats 1.2.0, src/windows.rs:23, uses WorkingSetSize, not PeakWorkingSetSize. This is not a continuously captured build peak: https://docs.rs/crate/memory-stats/1.2.0/source/src/windows.rs. Reference: crates/shamir-engine/examples/vector_report.rs:141; crates/shamir-engine/examples/vector_report.rs:265; Cargo.lock:2091.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- All 53 current claim statuses and nine plan statuses remain unchanged after independent inspection; none is source-fixed. Several Medium labels were recalibrated to Low for local-input, documentation, coverage, mirror-maintenance, and unmeasured setup-cost concerns.
- Material overview disagreement: SUMMARY and concurrency text still describe dependency implementation evidence as unavailable/unverified. The actual CARGO_HOME source and published peak_alloc 0.3.0 archive are present, and the archive hash matches the frozen lock.
- The parent reset-race confirmation is independently supported by the exact load/store versus fetch_add/fetch_max interleaving. Its cancellation-inheritance refutation is also supported by pre-existing reset-before-work code, not a later fix.
- The current vector_report dimension-filter qualification is correct but incomplete across callers: persisted_hnsw passes environment-controlled PH_DIM without rejecting zero.
- Effective-k reporting is not inherently a corrupted reproducibility key: positive-n requested k values above n collapse to the same generation. The actual recovery defect is missing requested provenance and generally unrecoverable sigma, not an invalid effective centroid count.
- The proposed endpoint fix leaves a separate half-open next_range violation. The proposed peak tests also need stale-peak discrimination and observable allocations, while Result followed by expect is not graceful operator-input handling.
- Exact allocator inspection exposes mandatory realloc copying and temporary double storage, beyond the reports' atomic-overhead framing. No quantitative slowdown, ratio, or RSS conclusion was fabricated.
- History and the source diff between the earlier reported snapshot and the required base show no relevant helper remediation. Earlier task/green labels were not treated as test-execution evidence.

## Current follow-up order

1. Make allocator instrumentation explicitly opt-in for linked consumers; disclose its realloc behavior and retain allocator configuration in comparisons.
2. Correct peak_mem's metric, activation, ownership, and isolation contracts; add isolated reset/window oracles and address the exact dependency reset race.
3. Validate local report parameters, including PH_DIM, and document/test both helper rejection paths and the sigma domain.
4. Repair next_f32's endpoint with an explicit fixture-compatibility decision; separately resolve next_range interpolation bounds.
5. Add independent Gaussian and integer-stream oracles plus source/generator identification; reconcile genuine mirrors without changing intentional variants.
6. Correct artifact recovery, Criterion, BENCH_QUICK, five-input, and nonportable-pointer documentation; move tests without losing registration.
7. Evaluate flat fixture storage as qualified setup optimization; preserve draw order, ownership boundaries, checked sizing, and historical output. Do not weaken the balance oracle without numerical evidence.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-bench-utils — Consolidated 7-lens review (synthesis of the 2026-08-14 cross-crate review)

Crate: `crates/shamir-bench-utils/` — a two-module bench-fixture helper (`vector_data`: seeded-LCG
clustered-dataset generator + `Lcg`; feature-gated `peak_mem`: global-allocator peak-RSS sampler),
narrowed to this shape after the 2026-07-07 Criterion migration (recorded in the crate's own
`src/lib.rs:9-12`).

Review basis: the seven 2026-08-14 lens reports for this crate, read in full and synthesized
read-only — `correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` (all under this directory). Structure/tone/rigor calibrated against the two
completed exemplars: `shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`.
Spot-checked against the crate source (`src/vector_data.rs`, `src/peak_mem.rs`, `src/lib.rs`,
`Cargo.toml`) and the named consumer sites (`shamir-engine/Cargo.toml:107`,
`shamir-index/Cargo.toml:64`, `examples/vector_report.rs`, the two live memory benches) — every
file:line cited below was verified. No builds, no tests, no source modifications. No new defects
were found during spot-checking.

Severity caveat: this is the supporting bench-fixture crate; nothing here is production-critical.
Severities are carried as tagged by the original lens reviewers. Same-root defects flagged by
multiple lenses are deduplicated to one entry under the primary lens (highest-severity treatment),
with the other lenses noted — matching the dedup convention of the workspace `SUMMARY.md`.

## Executive summary

A tiny, dependency-light crate whose entire value proposition is *number fidelity*: deterministic
byte-identical fixtures shared across benches/tests/reports, and honest peak-RSS samples. That is
exactly where its findings concentrate: (1) the `peak_mem` "**off by default**" isolation claim is
false as consumed — both in-repo consumers enable the feature unconditionally in
`[dev-dependencies]`, so every bench/example/test binary of shamir-engine and shamir-index
(including timing-only benches that never measure memory) silently runs under the `PeakAlloc`
global allocator, taxing recorded ns/op and invalidating any baseline that toggles the feature;
(2) the crate's other export — the LCG/Box-Muller determinism contract — is enforced by nothing:
the Box-Muller scale factor is pinned by no test (a transcription bug in `next_gaussian` would
pass the whole suite while mislabelling `sigma` in every published vector number) and the LCG
lineage is hand-mirrored ~13× across crates with zero golden enforcement; (3) the remaining mass
is hygiene: stale Criterion-era docs, an inline test module (the crate's single high), a
false "recoverable from the artefact" doc claim, and `assert!`-based validation with a
half-documented panic surface. Fix the allocator feature plumbing and add the determinism golden
pins before any further bench work builds on this crate.

---

## 1. correctness-tdd

### 1.1 — medium — `peak_mem` has zero test coverage; its load-bearing measurement contract rests on unpinned `peak_alloc` 0.3 semantics
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:1-111`; `crates/shamir-bench-utils/Cargo.toml:11,17`.
- Issue: the whole module is unverifiable: no tests anywhere (no `tests/` dir, no `#[cfg(test)]`),
  `doctest = false` in Cargo.toml, and every doc example is `rust,ignore` — nothing that exercises
  this code is ever compiled or run. Meanwhile its contract is load-bearing: `reset()`'s documented
  behavior ("reset the peak counter to the current allocation level") is
  `peak_alloc::PeakAlloc::reset_peak_usage()`'s semantics, and
  `crates/shamir-index/benches/create_index_streaming.rs:172-176` explicitly builds its published
  "PEAK HEAP" figure on it ("the raw figure includes the [data_store] baseline"). A `peak_alloc`
  0.3 → 0.4 semantics change (e.g. reset-to-zero) would silently flip the meaning of published
  peak-heap numbers with no test failure. Half the public API is also dead: `measure`,
  `measure_async`, and `current_allocated` have zero callers workspace-wide (only
  `setup`/`reset`/`current_peak` are used, by two benches), so they drift untested by
  construction. Against CLAUDE.md's Red/Green/Refactor protocol there is no failing-test-first
  path for any change here.
- Failure scenario: a dependency upgrade changes `reset_peak_usage` semantics → bench "PEAK HEAP"
  figures change meaning with no test failure; any edit to `peak_mem` ships with an entirely green
  suite regardless of correctness.
- Suggested fix: add `src/peak_mem/tests/` gated `#[cfg(all(test, feature = "peak_mem"))]`:
  measure a `vec![0u8; 1 << 20]` closure and assert `peak >= 1 MiB`; assert reset-to-current
  explicitly (allocate a known block, `reset()`, `current_peak() >= current_usage()`); assert
  `measure`/`measure_async` return the closure/future's result. Delete or test the dead API.
- Cross-lens: the zero-tests and dead-API facets are also flagged by error-handling-lifecycle
  (finding 6.2) and concurrency-lockfree (the module summary of `concurrency-lockfree.md`).

### 1.2 — medium — Box-Muller scale is unpinned: a transcription bug in `next_gaussian` would pass the entire suite
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:89-103` (vs tests `:217-363`).
- Issue: the crate's core distribution primitive has no golden-value and no statistical-moment
  test. Every existing test survives, e.g., dropping the `/s` term (`sqrt(-2 ln s)` instead of
  `sqrt(-2 ln s / s)`, shrinking the effective sigma by roughly 0.7× at s = 0.5):
  `points_are_clustered_not_scattered` (`intra < 0.25 * inter`) and `round_robin_balances_clusters`
  both still pass. Since `(k, sigma, seed)` is the documented reproducibility key that "surfaces
  in every report" (`vector_data.rs:29-30, 158-159`), a silently wrong scale mislabels every
  vector bench and recall/RSS report built on this shared fixture — exactly the cross-tool
  comparability the module exists to guarantee.
- Failure scenario: a refactor of `next_gaussian` alters the scale factor; all tests stay green;
  published recall-vs-sigma numbers become wrong relative to earlier reports.
- Suggested fix: add a fixed-seed golden test (first N `next_gaussian` values, exact `f32`
  equality) plus a coarse moment test (|mean| < 0.05 and std within ~5% of 1 over >= 50k draws).
  Same contract, second facet: the cross-crate mirror drift in finding 5.3 — one golden-stream
  test serves both.

### 1.3 — low — `Lcg::next_f32` violates its documented `[0, 1)` contract — can return exactly 1.0
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:71-76` (contract `:71`; downstream `:78-82, 92-95`).
- Issue: `(high as f32) / (1u64 << 32) as f32` rounds `u32 -> f32` to nearest; every `high` in
  `[4294967168, 4294967295]` (128 of 2^32 values, ~3e-8 per draw) rounds up to `2^32`, so the
  ratio is exactly `1.0`. This breaks `next_range`'s `[lo, hi)` doc (a centroid coordinate can be
  exactly `+1.0`) and the `[0, 1)` claim. `next_gaussian` happens to stay correct only because the
  `s < 1.0` acceptance check rejects `s = 1.0 + u2^2` — an undocumented, load-bearing accident.
  No test asserts the bound, and a naive sweep test would not catch it (needs ~3e7 draws);
  determinism itself is unaffected.
- Failure scenario: negligible for bench numbers, but the contract break is real and invisible to
  the suite; a future consumer relying on `x < 1.0` (e.g. slice indexing after scaling) has a
  one-in-four-billion off-by-one.
- Suggested fix: `(high >> 8) as f32 / (1u64 << 24) as f32` (exact division, `0.0 <= v < 1.0` by
  construction) — note this changes the stream mapping, so pair it with the format-version bump
  from finding 5.4 — and add a boundary test that crafts a seed whose next high-32 word is
  `0xFFFF_FFFF`.
- Cross-lens: also flagged by api-wire-protocol (finding 5.6, nit) with the same root and fix.

### 1.4 — nit — `round_robin_balances_clusters` asserts a statistical property as an exact equality
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:267-287`.
- Issue: the test requires every point's nearest centroid to equal its generating cluster
  (otherwise `counts != n / k`). The module doc itself (`:27-29`) says cross-target `f32`
  `ln`/`sqrt` identity is "not promised", so a near-tie in inter-centroid distances could flip one
  point's assignment on a different target and fail the exact assertion. Deterministic on a fixed
  target; brittleness is theoretical, but the test silently conflates "round-robin assigns
  balanced" with "nearest-centroid recovers the assignment".
- Failure scenario: a cross-target CI run flips one near-tie assignment → spurious red test, or
  (worse) the exact assertion gets loosened wholesale and stops pinning balance.
- Suggested fix: assert balance within a tolerance (e.g. every cluster count within 2 of `n / k`),
  or keep exact and note the cross-target caveat in the test.

### Cross-lens stubs (full write-ups elsewhere)
- **#4 (low, stale Criterion-era docs)** → primary at **7.2** (style-claude-md).
- **#5 (low, k-clamp asymmetry breaks artefact self-description)** → primary at **5.2** (api).
- **#6 (low, inline `#[cfg(test)] mod tests`)** → primary at **7.1** (style).
- **#7 (nit, `# Panics` omits `dim == 0`; `sigma` domain undocumented)** → primary at **6.1**
  (error-handling).
- **#8 (nit, Cargo.toml advertises removed BENCH_QUICK)** → primary at **5.5** (api).

## 2. concurrency-lockfree

**Pillar verdict: clean.** No `Mutex`/`RwLock`/`parking_lot`, no `scc`/`dashmap`/`ArcSwap`, no
hash-keyed structures at all (pillar 4 vacuous); the only synchronization is `peak_alloc`'s two
`Relaxed` `AtomicUsize` counters (lock-free, O(1) per op); the only `len()` calls are O(1)
`Vec::len()`, so the `clippy.toml` disallowed-methods ban is trivially satisfied. `Lcg` is a pure
value type, documented "no global state, no locking"; dataset generation is single-threaded by
design for the determinism contract. The one deduplicated defect below is bench-accuracy in
`peak_mem`'s process-global watermark — nothing memory-unsafe, nothing on a hot path, and the
module is feature-gated off by default (though see 5.1 for how "off" works out in practice). It
has zero tests, so these invariants are doc-guarded only; the consumers' `current_thread`-runtime
pattern is the actual enforcement today.

### 2.1 — low — `peak_mem`'s measurement window is unowned: concurrent or aborted use silently corrupts the one process-global peak watermark
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:57-59` (`reset`), `:71-93` (`measure`),
  `:95-110` (`measure_async`, caveat only at `:98-101`).
- Issue (four facets of one root — the reset→read window has no owner, guard, or detection):
  1. **`reset()` TOCTOU** (concurrency #1): `reset()` delegates to
     `PeakAlloc::reset_peak_usage()`, which (verified against peak_alloc 0.3.0 source,
     `lib.rs:108-110`) is `PEAK.store(CURRENT.load(Relaxed), Relaxed)` — two independent atomic
     ops, not one CAS. An `alloc` on another thread landing between the load and the store
     performs `CURRENT.fetch_add` + `PEAK.fetch_max` (`:124-128`), and the subsequent store erases
     that contribution — under-reporting, load-dependently (two runs can disagree).
  2. **Overlapping `measure`/`measure_async`** (concurrency #2, performance #2): two overlapping
     calls — two tasks on a multi-threaded runtime, or `measure_async` interleaved with any other
     `reset()` — destroy each other's baseline: the second `reset()` erases the first's, and both
     `current_peak()` reads return a merged maximum (silent under- *and* over-report). The number
     is published as a heap watermark with no error — precisely the artifact class the F-78/F-53a
     reports are built on. Dormant today: no workspace caller of `measure`/`measure_async` exists;
     both live consumers drive exactly one workload between `reset()` and `current_peak()` on a
     `new_current_thread` runtime (`create_index_streaming.rs:165-193`, `streaming_topk.rs:113-133`).
  3. **Doc asymmetry** (concurrency #3): `measure` carries none of the concurrency caveats
     `measure_async` has (foreign-task pollution on a multi-threaded executor), yet the
     module-level example demonstrates `measure` inside `b.to_async(&rt)` — exactly the shape
     where the caveat applies.
  4. **No drop-guard** (error-handling #3): both helpers reset process-global state and capture
     the peak only on the success path. If `f` panics or the future is dropped mid-`await`
     (harness timeout/cancellation — the realistic case for the async variant), the counter stays
     "armed" from the stale reset, so the *next* measurement silently includes the aborted cell's
     allocations from a wrong baseline.
  Additionally (api #9): the single-flight constraint is stated nowhere;
  `setup()`'s rationale ("so the linker doesn't strip the global allocator in LTO builds",
  `:49-50`) is not how `#[global_allocator]` registration works — an honest no-op with a
  mis-explaining doc (that clause also belongs to finding 5.1's activation-model problem); and
  `measure` returns an anonymous `(R, usize)` tuple where a named struct would be
  self-documenting.
- Failure scenario: a future bench measures two workloads concurrently, or drops a timed-out
  `measure_async` future; the following cell's peak is a merged maximum or includes the abandoned
  cell's tail allocations — a corrupted published RSS/peak table with zero error signal, feeding a
  wrong `/opti` baseline-vs-after conclusion.
- Suggested fix: one mechanism covers the hazards — an `AtomicBool` "measurement in flight" claim
  (`compare_exchange` on `reset`/`measure`/`measure_async` that fails loudly on re-entry) plus a
  `PeakGuard` (captures-and-restores on drop) held across `f`/`f.await`. At minimum, hoist one
  concurrency section into the module docs covering `setup`/`reset`/`measure`/`measure_async`/
  `current_peak` uniformly: process-global counters, at most one measurement in flight
  process-wide, `current_thread` runtime per measurement (link the two live benches as the
  reference pattern). Consider `struct Measurement<R> { result: R, peak_bytes: usize }`.
- Cross-lens: performance-hotpath #2 and error-handling-lifecycle #3 are the same root (flagged
  by three lenses); api-wire-protocol #9's single-flight clause folds in here.

### Cross-lens stubs (full write-ups elsewhere)
- **#4 (nit, `#[global_allocator]` shipped from a library crate)** → primary at **5.1** (api).

## 3. security-crypto

**Verdict: outside the crypto boundary, effectively clean.** No auth/HMAC/SCRAM/TLS code, zero
`unsafe`, zero `static mut`, no file/network/process/env surface (grep-verified across `src/`).
The only randomness is `Lcg` (`vector_data.rs:52-104`), explicitly documented as **not**
cryptographically secure (`:32-38`) and used solely for deterministic bench fixture data; both
workspace consumers declare the crate under `[dev-dependencies]` only
(`shamir-engine/Cargo.toml:107`, `shamir-index/Cargo.toml:64`), so neither the LCG nor the
feature-gated allocator can reach a production build (shamir-index's tests deliberately
re-implement the LCG rather than import it — see finding 5.3 — further confirming the library
path never touches it). No injection surface, no timing-side-channel surface, no secrets;
`peak_alloc` pinned at 0.3.0 with a registry checksum (`Cargo.lock:2395-2399`).

### 3.1 — low — *(primary: same as 5.1)* — feature-gated `#[global_allocator]` in a library crate is a process-wide side effect
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:39-40` (exported via `src/lib.rs:14-15`).
- The security lens's framing of the 5.1 root: enabling `peak_mem` compiles `PeakAlloc` into the
  library itself; an allocator is a *process-global* property of the final binary, so any binary
  linking this crate with the feature on gets the tracking allocator around **every** allocation —
  the module doc ("normal `cargo bench` paths are unaffected", `peak_mem.rs:3-4`) understates it.
  Failure mode is drift, not today: a future runtime dependency (or released binary) enabling
  `peak_mem` ships an allocation-tracking allocator — per-alloc atomic overhead, globally
  contended counters — with no build-time signal (a conflicting in-binary allocator fails loudly
  at E0152/E0159). Fix options at 5.1; the security lens adds: export a `declare_peak_alloc!()`
  macro (or documented snippet) each *bench binary* pastes so `#[global_allocator]` lives in the
  bench file, not the library, plus an explicit "dev-dependency only" warning.

## 4. performance-hotpath

**Verdict:** no production hot path exists; O(x→0) exposure is confined to fixture-generation
cost and the fidelity of the measurements this crate produces. No hidden O(N²), no unbounded
growth/buffering. One substantive theme hit (4.1); the other two lens findings dedupe into 2.1
and 7.2.

### 4.1 — medium — Fixture generator allocates one heap `Vec` per point (allocation-in-loop: n+1 allocations per call)
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:199-207` (loop; `Vec::with_capacity(dim)`
  at `:202`), `:115` (`ClusteredDataset::vectors: Vec<Vec<f32>>`).
- Issue: `clustered_vectors` builds the dataset as `Vec<Vec<f32>>`, allocating per point inside
  the loop — exactly the "allocation in loops" shape pillar 3 bans in helpers. Every call pays n
  separate heap allocations (plus the outer one), 24-byte per-`Vec` headers, allocator rounding,
  and heap fragmentation; the flat payload is scattered across the heap.
- Failure scenario: `benches/vector_search.rs:93` runs an optional n = 1_000_000 rung
  (`BENCH_VECTOR_1M`) at dim up to 768 — 1M allocations and ~24–40 B/vector of pure metadata
  overhead (≈8% of the payload at dim=128) before any index bytes. `examples/vector_report.rs`
  publishes RSS footprints computed over this structure, so reported per-dataset memory
  overstates actual vector bytes and varies with allocator behaviour, weakening the OLD-vs-NEW
  comparability the tool exists for. Locality of consumers that iterate the cloud (brute-force
  ground truth, and `benches/vector_search.rs:121-126`, which re-`clone`s every boxed row into a
  second boxed batch — each point copied twice, twice fragmented) is also degraded. Setup-only,
  so timed latencies are unaffected; the skew lands in memory reports and wall-clock setup.
- Suggested fix: store a flat `Vec<f32>` slab of `n * dim` in `ClusteredDataset` with
  `row(i) -> &[f32]` accessors. Generation order is already row-major, so the LCG call sequence
  is unchanged and `(n, dim, k, σ, seed)` fixtures stay byte-identical (add a
  `same_seed_is_byte_identical` assertion against the current layout to pin it). Keep the
  boxed-rows shape available as an explicit `to_vec_rows()` for consumers that need owned rows,
  so HNSW-ingestion call sites opt into the copy. The flat slab makes the n-allocations-per-call
  property structurally impossible to regress. Pairs naturally with the params/version metadata
  of findings 5.2/5.4 (same struct change).

### Cross-lens stubs (full write-ups elsewhere)
- **#2 (low, global watermark overlap + missing sync-variant warning)** → primary at **2.1**.
- **#3 (nit, docs teach the removed Criterion integration)** → primary at **7.2** (style).

## 5. api-wire-protocol

Tiny, dependency-free public surface; no `serde`/`serde_json` anywhere, so the builder-only
query rule is trivially satisfied. The crate's de-facto "wire protocol" is the byte-identical
determinism contract of `clustered_vectors` — and that is where the findings concentrate.

### 5.1 — medium — `peak_mem`'s allocator is silently active for every bench/example/test binary of shamir-engine and shamir-index, contradicting the documented "off by default" contract
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:3-8` (contract claim), `:34`, `:44-52`
  (activation-model claims), `:39-40`; `crates/shamir-bench-utils/Cargo.toml:13-14`; enabled
  unconditionally at `crates/shamir-engine/Cargo.toml:107` and `crates/shamir-index/Cargo.toml:64`.
- Issue: the module doc promises the feature is "**off by default** so normal `cargo bench` paths
  are unaffected", and the `#[global_allocator]` static is commented "Activated by calling
  `setup` once" (`:34`). Neither is true. Both consumers declare
  `shamir-bench-utils = { path = ..., features = ["peak_mem"] }` unconditionally in
  `[dev-dependencies]`, and with workspace resolver 2 dev-dependency features are shared across
  *all* dev targets of the declaring package — the allocator activates at link time, not when
  `setup()` (an honest no-op whose linker comment mis-describes `#[global_allocator]`
  registration; also flagged by api #9) is called. So every bench, example, and **unit-test**
  binary in shamir-engine and shamir-index links `PeakAlloc` — including timing-only benches that
  merely use `vector_data` fixtures (e.g. `crates/shamir-engine/benches/filtered_vector_search.rs:25`)
  and never measure memory. Those benches pay `PeakAlloc`'s per-allocation atomic accounting in
  their *timing* numbers (an overhead `peak_mem.rs:37-38` itself acknowledges). The security lens
  (3.1) adds the structural framing: a process-global allocator compiled into a *library*; the
  concurrency lens (its #4, nit) adds the E0152 conflict with any consumer binary defining its
  own allocator, e.g. the workspace's allocator switch
  `crates/shamir-db/benches/bench_allocator.rs:8-25` — today dodged only by convention, noted in
  each consumer (`create_index_streaming.rs:24`) rather than where the allocator is defined; the
  error-handling lens (its #4, low) adds that `./scripts/test.sh` runs of two core crates execute
  their entire unit-test graphs under `PeakAlloc` with no opt-in and no kill switch, in contrast
  to shamir-engine's own careful `test-util` blast-radius documentation (`shamir-engine/Cargo.toml:86-91`).
- Failure scenario: an `/opti` baseline-vs-after pair, or a workspace-wide bench sweep, mixes
  allocator-on and allocator-off cells; recorded ns/op carry a hidden allocator tax, and any
  consumer toggling the feature between runs invalidates every recorded baseline with no signal —
  precisely the comparability corruption the crate exists to prevent. Longer term: a future
  regular (non-dev) dependency edge with the feature pushes `PeakAlloc` into a shipped binary
  unnoticed.
- Suggested fix: remove `features = ["peak_mem"]` from both consumer manifests; instead add
  `peak_mem = ["shamir-bench-utils/peak_mem"]` to each consumer's own `[features]` and tag only
  the memory benches with `required-features = ["peak_mem"]`, so the allocator is compiled in
  solely when opted in per invocation — or split the allocator into its own micro-crate (or a
  paste-in `declare_peak_alloc!()` macro, per the security lens) so fixture-generation consumers
  can never unify it in. Correct the module doc to describe the real link-time/unification
  semantics either way, and add a one-line comment at both consumer dep sites.

### 5.2 — medium — `ClusteredDataset` doc claims "(k, σ) parameters are recoverable from the artefact alone" — σ never is, and k is clamped
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:108-111` (claim) vs `:113-118`
  (fields), `:153-154` + `:179-183` (clamp); contradicts `:29-30` and `:156-159`.
- Issue: the struct carries only `vectors` and `centroids`. `sigma` and `seed` appear nowhere in
  the artefact, and `k()` returns `centroids.len()`, which is `k_eff = min(k_clusters, n)` — not
  the requested `k_clusters` (the `k_greater_than_n_clamps_silently` test at `:332-339` bakes in
  `ds.k() <= 3` for a request of `k = 10`). The module doc (`:29-30`) and fn doc (`:156-159`)
  simultaneously instruct callers to "surface" the full five-value key in reports — i.e. the
  values must be threaded externally — directly contradicting the struct doc's recoverability
  claim. The one report tool today works around it by defining its own `DatasetParams` struct
  (`crates/shamir-engine/examples/vector_report.rs:180-190`, used to print the header at
  `:336-338`), an admission that the artefact is *not* the single source of truth the doc
  promises. The correctness lens (its #5, low) adds the clamp asymmetry facet: for `n > 0,
  k_clusters > n` the requested `k` is discarded, while the `n == 0` path deliberately preserves
  all `k_clusters` (`:176-183`) — and the fn doc's blanket "`k_clusters > n` clamps to `n`"
  (`:153-154`) does not carve out the `n == 0` path, so the artefact self-description fails
  exactly in the clamped regime.
- Failure scenario: a future tool trusts the struct doc, prints `ds.k()` and omits σ/seed:
  clamped-k reports get labelled with a `k` that never generated anything, and the printed
  "reproducibility key" cannot reproduce the data — the exact failure mode this crate was
  extracted to prevent.
- Suggested fix: make the artefact actually self-describing — add request metadata to the struct
  (e.g. `pub params: DatasetParams { n, dim, k_clusters, sigma, seed }`, mirroring the consumer's
  own `DatasetParams`) — or reword both doc sites to the truth (only `n`, `dim`, `k_eff` are
  recoverable; σ/seed/requested-k must be threaded by the caller). One struct change also lands
  the flat slab (4.1) and the format version (5.4).

### 5.3 — medium — The cross-crate LCG "lineage" contract is prose-only: ~13 hand-maintained mirrors, zero enforcement, and the stated justification for the duplication is stale
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:34-36` and `:40-44` (contract by
  comment); mirrors e.g. `crates/shamir-index/src/vector/tests/sq8_tests.rs:15-16,33,46-47`,
  `quantized_dist_tests.rs:36,49`, `quantized_graph_tests.rs:37,50`, `compaction_tests.rs:24,744`,
  `quantization_snapshot_tests.rs:48,88`, `hnsw_rs_contract_tests.rs:36,41`, `snapshot_tests.rs:40`,
  `vector_restore_tests.rs:80`, `delta_log_tests.rs:56`, `crash_recovery_tests.rs:98`,
  `cofilter_prefilter_tests.rs:36`, `hnsw_adapter_tests.rs:19`, `deadlock_regression_tests.rs:89`,
  plus `crates/shamir-engine/benches/vector_bulk_compaction.rs:96`.
- Issue: the crate's central export is a *determinism contract*: the constant, the high-32
  extraction, the polar Box-Muller consumption order. The doc names one mirror
  (`hnsw_rs_contract_tests::lcg_vec`, which does exist) but at least 13 sites re-implement parts
  of it "mirroring `shamir_bench_utils`" by comment alone. No shared constant and no golden-stream
  test assert the mirrors still match. Worse, `sq8_tests.rs:15-16` justifies the duplication with
  "because `shamir-bench-utils` is not a dev-dependency of this crate" — false today:
  `crates/shamir-index/Cargo.toml:64` declares it as a dev-dependency, so the tests could consume
  `shamir_bench_utils::{Lcg, clustered_vectors}` directly.
- Failure scenario: a one-sided edit — a different Box-Muller variant, returning both variates,
  changing the bit-extraction width — silently breaks byte-identical lineage. Contract-test
  fixtures and bench data stop corresponding with no failing test, and cross-tool recall
  comparisons (the stated purpose, `vector_data.rs:3-7`) become apples-to-oranges.
- Suggested fix: since the dev-dependency now exists, delete the mirrors in shamir-index tests and
  use `shamir_bench_utils::{Lcg, clustered_vectors}`; where a copy must remain, pin it with a
  golden test asserting the first N stream values documented in one place; expose
  `pub const LCG_MULT`/`LCG_INC` so the constant has a single canonical address. Same contract,
  in-crate facet: finding 1.2 — one golden-stream test serves both.

### 5.4 — low — Reproducibility key excludes any generator/dataset-format version
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:21-30`, `:156-159`.
- Issue: the documented key is `(n, dim, k_clusters, sigma, seed)`, but the byte stream is as much
  a function of the *code version* as of the key: generation order, Box-Muller variant, and
  centroid-draw order all feed it. Nothing in the public API identifies the algorithm, and
  consumers print only the key in report headers (`vector_report.rs:336-338`).
- Failure scenario: any generation-affecting change (including the fixes proposed for 1.3 and
  4.1!) silently produces different data under the same key; every previously recorded comparison
  keyed on the five-tuple becomes unreproducible with no warning.
- Suggested fix: add `pub const DATASET_FORMAT_VERSION: u32` (bumped on any generation-affecting
  change), carry it on `ClusteredDataset` (see 5.2), and print it in report headers next to the
  tuple.

### 5.5 — low — Cargo.toml `description` advertises the removed BENCH_QUICK tier feature
- File:line: `crates/shamir-bench-utils/Cargo.toml:6` vs `crates/shamir-bench-utils/src/lib.rs:9-12`.
- Issue: the package description still reads "BENCH_QUICK env-var support for /opti
  baseline/after pairs" — that is the removed Criterion-era API. The crate's actual surface today
  is vector fixtures + optional peak-RSS sampling. Metadata drift on the crate's public face
  (`publish = false`, but it is still the first thing a reader sees in the manifest).
- Failure scenario: a contributor greps manifests for "BENCH_QUICK", concludes the env-var
  support lives here, and wires against a feature that no longer exists.
- Suggested fix: update the description to match the crate (e.g. "Shared bench helpers:
  deterministic clustered vector datasets and optional peak-RSS sampling").
- Cross-lens: also flagged by correctness-tdd (#8, nit) with the same root.

### Cross-lens stubs (full write-ups elsewhere)
- **#5 (low, peak_mem docs teach the removed Criterion API)** → primary at **7.2** (style).
- **#6 (low, `clustered_vectors` panics instead of returning `Result`)** → primary at **6.1**
  (error-handling).
- **#8 (nit, `Lcg::next_f32` can return 1.0)** → primary at **1.3** (correctness).
- **#9 (nit, undocumented single-flight contract; `setup()` rationale dubious; tuple return)** →
  primary at **2.1** (concurrency), with the `setup()` activation-model clause folded into **5.1**.

## 6. error-handling-lifecycle

The crate is almost entirely infallible-by-construction: no `Result`, `thiserror`, or `anyhow`
anywhere, and most APIs cannot fail. The one genuinely fallible public API is the subject of 6.1;
the `peak_mem` lifecycle findings dedupe into 2.1 and 5.1.

### 6.1 — medium — `clustered_vectors` validates arguments with `assert!` instead of `Result`/`thiserror`, one caller feeds it externally-controlled input, and the second panic path (`dim == 0`) is undocumented and untested
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:171-172` (asserts) vs `:161-163`
  (`# Panics` doc); test gap at `:341-345`; caller:
  `crates/shamir-engine/examples/vector_report.rs:406` → `:212`.
- Issue (one decision point, three filed facets):
  1. **`assert!` vs `Result`** (error-handling #1, medium): the crate's only fallible API signals
     invalid input via `assert!(k_clusters > 0, ...)` / `assert!(dim > 0, ...)`. CLAUDE.md's
     error-handling rules require `Result<T, E>` with `thiserror` for library error enums,
     reserving panics for invariant violations. One can argue a misconfigured bench fixture is a
     programmer bug — but the panic is reachable from *user* input, not just code:
     `vector_report.rs:406` reads `env_usize("VR_K_CLUSTERS", 64)` and passes the parsed value
     straight into `clustered_vectors` at `:212`. A successfully parsed `0` (env var set to `"0"`)
     sails past the `unwrap_or(default)` fallback and detonates the assert deep inside the tool.
     (Tempering factors: the panic is documented in `# Panics`, covered by a `#[should_panic]`
     test, and all bench callers pass literals.) The api lens (#6, low) adds the trajectory
     framing: the parameters are runtime-shaped values and the report tooling is already drifting
     toward parameterised runs threaded through `DatasetParams`.
  2. **`dim == 0` missing from `# Panics`, no test** (error-handling #2, medium): the doc promises
     only "Panics if `k_clusters == 0`", but `:172`'s `assert!(dim > 0, ...)` is a second,
     unmentioned panic path. The error-path suite covers exactly one of the two cases
     (`zero_clusters_panics`, `:341-345`); there is no `#[should_panic(expected = "dim must be >
     0")]` counterpart — a doc-driven caller has no way to know `dim = 0` is fatal.
  3. **`sigma` domain undocumented** (correctness #7, nit): negative sigma mirrors the noise
     (harmless but undocumented); NaN sigma poisons the whole dataset silently; no validation, no
     doc.
- Failure scenario: `VR_K_CLUSTERS=0 cargo run --release --example vector_report` builds a tokio
  runtime and starts the report pipeline, then panics mid-run with a bare `"k_clusters must be >
  0"` instead of a clean validation message and exit code; a table-driven sweep with a mis-parsed
  empty dims list panics on an input the documentation said could not occur.
- Suggested fix: change the signature to `-> Result<ClusteredDataset, VectorDataError>` with a
  small `thiserror` enum (`#[error("k_clusters must be > 0")] ZeroClusters`, same for `ZeroDim`);
  update the ~8 bench/example call sites to `.expect(...)` at their own boundaries (anyhow/expect
  is sanctioned in binaries/tests). If the panic stance is deliberately kept for this bench-only
  helper, it must at least be internally consistent: document **both** panic paths (plus sigma's
  expected domain) and add the missing `#[should_panic]` test for `dim == 0`. Validate at the
  env/CLI boundary if/when parameters become externally supplied.
- Cross-lens: api-wire-protocol #6 (low) and correctness-tdd #7 (nit) are the same root.

### Cross-lens stubs (full write-ups elsewhere)
- **#3 (low, `measure`/`measure_async` no drop-guard on panic/cancellation; zero tests)** →
  primary at **2.1** (concurrency).
- **#4 (low, allocator silently installed into every consumer test/bench/example binary;
  `setup()` misrepresents activation)** → primary at **5.1** (api).
- **#5 (nit, inline tests, which is where the error-path gap lives)** → primary at **7.1** (style).

## 7. style-claude-md

Structurally minimal (3 source files, no `mod.rs` needed) and otherwise clean: `lib.rs` is
declaration-only, every `use` sits at file top (the sole `use super::*;` is the documented test
exception), `peak_mem.rs` is one closely-coupled group, and all doc cross-references resolve to
real files. Two real problems (7.1, 7.2) plus two judgment-call nits.

### 7.1 — high — Entire test module embedded inline in `vector_data.rs`, violating the mandatory `tests/` layout
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:217-363`;
  `crates/shamir-bench-utils/Cargo.toml:10`.
- Issue: CLAUDE.md § "Test organisation" rule 5 is unambiguous: *"**Never embed `#[cfg(test)] mod
  tests { ... }` inline** inside implementation files. Move them to the `tests/` directory."*
  This file carries its full 9-test suite (~150 lines: determinism, clustering-shape, clamping,
  LCG-stream tests) as an inline `#[cfg(test)] mod tests` block — the exact pattern the rule
  forbids. The crate's `Cargo.toml` comment ("Tests live alongside their callers; this crate is a
  thin helper", `Cargo.toml:10`) reads as a self-granted exception, but conventions come from
  CLAUDE.md only, which sanctions no per-crate carve-out; sibling crates
  (`shamir-index/src/vector/tests/*.rs`) follow the mandated layout. The error-handling lens (its
  #5, nit) adds the substantive cost: the inline block is where the missing `dim == 0` error-path
  test (6.1) and the absent `peak_mem` coverage (1.1) would have to live, and the same
  `Cargo.toml` comment conflates `doctest = false` with unit-test placement — only the former is
  a real setting.
- Failure scenario: test files don't get topic-split or discovered the way the convention
  guarantees; `git blame` on implementation vs. tests entangles; the deviation normalizes
  copy-paste of inline tests into other crates.
- Suggested fix: convert to `src/vector_data/mod.rs` (module declarations + re-exports only),
  implementation in a sibling file, and split tests into
  `src/vector_data/tests/{mod.rs, lcg_tests.rs, clustered_vectors_tests.rs}` with a re-export-only
  `tests/mod.rs` manifest, matching the documented layout; add the feature-gated
  `src/peak_mem/tests/` home at the same time. Drop the `Cargo.toml` comment that codifies the
  exception (or record the deviation as an explicit workspace exception — none currently exists).
- Cross-lens: also flagged by correctness-tdd (#6, low) and error-handling-lifecycle (#5, nit).

### 7.2 — medium — `peak_mem.rs` docs still teach Criterion as the module's usage pattern — the harness the workspace removed and banned — and `vector_data.rs` still cites "the criterion bench"
- File:line: `crates/shamir-bench-utils/src/peak_mem.rs:10-30` (module doc: "# Usage with
  Criterion `iter_custom`" + full example), `:15` (`criterion_main`), `:18-19`, `:44` ("before
  `criterion_main!` or equivalent"); `crates/shamir-bench-utils/src/vector_data.rs:3`.
- Issue: CLAUDE.md is emphatic that the workspace migrated off Criterion on 2026-07-07: benches
  use `bench_scale_tool::Harness`, Criterion APIs "no longer apply to this repo" and must not be
  reached for; the crate's own `lib.rs:9-12` records the removal. Yet this module's primary usage
  documentation is a Criterion `iter_custom` recipe (plus the report-bytes-as-`Duration::from_nanos`
  trick), and `setup()`'s doc says the same. No `criterion` dependency exists anywhere under
  `crates/*/Cargo.toml`, and the module's actual live consumers
  (`create_index_streaming.rs:179-197`, `streaming_topk.rs:114-127`) call
  `setup()`/`reset()`/`current_peak()` directly beside `bench_scale_tool::Harness` — the pattern
  the docs never show. `vector_data.rs:3` still calls `benches/vector_search.rs` "the criterion
  bench" (style #3, low), contradicting the referenced file's own header ("Migrated to the
  fixed-iteration harness", `vector_search.rs:46-49,53`). Compounding it, `measure`,
  `measure_async`, and `current_allocated` (`peak_mem.rs:67-110`) — whose only documented use is
  that Criterion example — have zero callers workspace-wide (grep confirms references only inside
  this file's doc comments). The examples are `rust,ignore` with `doctest = false`, so nothing
  compile-checks them: the api lens (#5, low) notes a new bench author copying the module's own
  docs is steered straight at APIs that no longer exist in this repo, and the correctness lens
  (#4, low) and performance lens (#3, nit) flag the same rot from their angles (a copied example
  can also wire `measure` into a harness whose measured region doesn't match the reset window —
  numbers attributed to the wrong region).
- Failure scenario: a contributor adding peak-mem sampling copies the module doc example and
  reintroduces a Criterion dev-dependency — exactly the "repeatedly-forgotten convention"
  CLAUDE.md warns about — or wires a measurement whose region doesn't match the reset window;
  the dead `measure`/`measure_async` API lingers as untested public surface documented against a
  harness that cannot compile here.
- Suggested fix: rewrite the module doc and `setup()` doc around the real integration: enable the
  `peak_mem` feature, call `setup()`, bracket workload with `reset()`/`current_peak()` inside a
  `bench_scale_tool::Harness` bench (copy the `create_index_streaming.rs` pattern; template bench:
  `crates/shamir-engine/benches/tx_pipeline.rs`). Drop the word "criterion" from
  `vector_data.rs:3`. Delete or explicitly mark `measure`/`measure_async`/`current_allocated` as
  currently unused (or remove them) rather than documenting them via a banned harness.
- Cross-lens: also flagged by correctness-tdd (#4, low), performance-hotpath (#3, nit), and
  api-wire-protocol (#5, low) — four lenses, one root.

### 7.3 — low — `vector_data.rs` carries three public exports (`Lcg`, `ClusteredDataset`, `clustered_vectors`) — borderline against one-file-one-export
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:52` (`pub struct Lcg`), `:113`
  (`pub struct ClusteredDataset`), `:164` (`pub fn clustered_vectors`).
- Issue: CLAUDE.md's discipline rule: one file = one primary export, or a *closely-coupled* group;
  multiple *unrelated* public types must be split. The borderline call is `Lcg`: it is a fully
  self-contained, general-purpose RNG value type (own doc, own constants, mirrored independently
  in five `shamir-index/src/vector/tests/*` `lcg_vec` helpers — see 5.3), not inherently tied to
  dataset generation; `ClusteredDataset` + `clustered_vectors` are genuinely one coupled unit.
  Grouping all three is defensible for a 2-module crate, but `Lcg` would sit more cleanly in its
  own `lcg.rs` per the rule's letter and its `git blame` rationale.
- Failure scenario: none at runtime — a structural-hygiene judgment call, flagged because the
  rule is documented as strict.
- Suggested fix: if the crate grows at all, move `Lcg` (plus `LCG_MULT`/`LCG_INC`, which 5.3
  proposes making `pub` anyway) to `src/lcg.rs` and keep `vector_data.rs` to the generator +
  result type. Acceptable to defer while the crate stays this small — but then note the coupling
  rationale in the module doc.

### 7.4 — nit — Reproducibility key described inconsistently: "(k, σ, seed) triple" vs. five values
- File:line: `crates/shamir-bench-utils/src/vector_data.rs:29` ("The `(k, σ, seed)` triple is the
  reproducibility key") vs. `:158-159` ("Same `(n, dim, k_clusters, sigma, seed)` → byte-identical
  ... Surface these five values").
- Issue: the module-level doc calls a "triple" the reproducibility key while the function doc
  correctly requires all five parameters (output depends on `n` and `dim` too, so the triple alone
  is insufficient). The same triple-vs-five slip appears in the downstream bench doc
  (`vector_search.rs:19`), suggesting the drift propagated from here.
- Failure scenario: a published report surfaces only `(k, σ, seed)` per the module doc and the
  dataset — hence the recall numbers — is not reproducible.
- Suggested fix: align both doc sites on the five-value key `(n, dim, k_clusters, σ, seed)`; fix
  the downstream mention opportunistically in the owning crate. Natural to land together with
  5.2's params-on-struct change.

---

## Finding counts

Raw lens-tagged findings across the seven files: **36** (matches the workspace SUMMARY's
pre-dedup per-crate row: 0 crit / 1 high / 9 med / 16 low / 10 nit). After deduplicating
same-root defects flagged by multiple lenses: **16 distinct defects**.

| Severity | Lens-tagged findings | Distinct defects (deduped) | Deduped finding numbers |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 1 | 1 | 7.1 |
| medium | 9 | 8 | 1.1, 1.2, 4.1, 5.1, 5.2, 5.3, 6.1, 7.2 |
| low | 16 | 5 | 1.3, 2.1, 5.4, 5.5, 7.3 |
| nit | 10 | 2 | 1.4, 7.4 |
| **total** | **36** | **16** | |

Deduplicated groups (members are cited by their original lens + number):

| Deduped finding | Severity | Members (lens #) | Lenses flagging it |
|---|---|---|---|
| 7.1 inline test module | high | style #1, correctness #6, error #5 | style, correctness, error |
| 7.2 stale Criterion docs | medium (tags: med/low/low/nit/low) | style #2 + #3, correctness #4, perf #3, api #5 | style, correctness, perf, api |
| 6.1 assert-based validation + `dim == 0` gap | medium (tags: med/med/low/nit) | error #1 + #2, api #6, correctness #7 | error, api, correctness |
| 5.1 allocator link-time blast radius | medium (tags: med/low/low/nit) | api #1, error #4, security #1, concurrency #4 | api, error, security, concurrency |
| 5.2 artefact not self-describing | medium (tags: med/low) | api #2, correctness #5 | api, correctness |
| 2.1 unowned measurement window | low (tags: low/low/low/nit/low/nit) | concurrency #1 + #2 + #3, perf #2, error #3, api #9 (part) | concurrency, perf, error, api |
| 1.1 `peak_mem` zero tests + unpinned `peak_alloc` semantics | medium | correctness #1 (zero-tests/dead-API facets also noted by error #3, concurrency summary) | correctness, error, concurrency |
| 5.5 BENCH_QUICK description | low (tags: low/nit) | api #7, correctness #8 | api, correctness |
| 1.3 `next_f32` returns 1.0 | low (tags: low/nit) | correctness #3, api #8 | correctness, api |
| standalone | — | correctness #2, #9 · perf #1 · api #3, #4 · style #4, #5 | one lens each |

Note on deduped severities: where the tags within a group differ, the deduped entry carries the
**maximum** tag (the primary lens's), with the spread noted — matching how the workspace SUMMARY's
systemic-pattern entries cite "(med, low)" for this crate's multi-lens roots.

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Make `peak_mem` actually opt-in per invocation.** Remove `features = ["peak_mem"]` from both
   consumers' `[dev-dependencies]`; add `peak_mem = ["shamir-bench-utils/peak_mem"]` passthrough
   features to shamir-engine/shamir-index and tag only the memory benches with
   `required-features = ["peak_mem"]` (or isolate the allocator into a micro-crate / paste-in
   `declare_peak_alloc!()` macro). Correct the module doc ("off by default", "Activated by calling
   `setup` once", the LTO/linker story) to state the link-time, dev-target-wide reality and the
   E0152 interaction, and annotate both consumer dep lines. Closes **5.1** (with the 3.1, 6.3,
   and concurrency-#4 framings).
2. **Pin the determinism contract with golden tests.** Fixed-seed golden values for
   `next_gaussian` (first N draws, exact `f32`) + a coarse moment test; expose
   `pub const LCG_MULT`/`LCG_INC`; migrate the ~13 shamir-index mirrors onto
   `shamir_bench_utils::{Lcg, clustered_vectors}` (the dev-dependency exists) or golden-pin the
   copies; add `DATASET_FORMAT_VERSION` so any generation-affecting change (including the fixes
   below) is visible in report headers. Closes **1.2** and **5.3**, adds the enforcement axis for
   **5.4**.

**P1 — soon**

3. **Make `ClusteredDataset` self-describing and flat.** Store requested params
   (`n, dim, k_clusters, sigma, seed`) + `DATASET_FORMAT_VERSION` on the struct (or fix the doc to
   the truth); switch to a flat `Vec<f32>` slab with `row(i)` accessors + explicit `to_vec_rows()`,
   pinning byte-identity with a golden assertion. Closes **5.2**, **4.1**, lands **5.4**; also
   fixes the 7.4 "triple vs five" doc drift in the same pass.
4. **Test and own the `peak_mem` measurement window.** Feature-gated unit tests pinning
   `reset`/`current_peak`/`measure` semantics; `AtomicBool` in-flight claim + `PeakGuard`
   drop-guard; one module-doc concurrency section (process-global, serial-only,
   `current_thread` per measurement, live benches as reference); delete or implement the dead
   `measure`/`measure_async`/`current_allocated` API. Closes **1.1** and **2.1**.
5. **Move the tests to the mandated layout.** `src/vector_data/tests/` (+ feature-gated
   `src/peak_mem/tests/`), manifest-only `mod.rs`, implementation split per one-file-one-export;
   drop the self-granted `Cargo.toml:10` exception comment. Closes **7.1**; gives 6.1's missing
   error-path test and 1.1's coverage a home.
6. **Rewrite the Criterion-era docs** against `bench_scale_tool::Harness` (module example +
   `setup()` doc + `vector_data.rs:3`). Closes **7.2**.
7. **Resolve the panic-path decision.** `Result` + `thiserror` for `clustered_vectors` with
   `.expect()` at the ~8 bench/example call sites — or, if the panic stance is kept, document both
   panic paths + sigma domain and add the missing `#[should_panic]` test. Either way, validate
   `VR_K_CLUSTERS`-style env input at the boundary. Closes **6.1**.

**P2 — backlog**

8. **`next_f32` exact 24-bit conversion** (`(high >> 8) / 2^24`) + boundary test, paired with the
   `DATASET_FORMAT_VERSION` bump from item 2. Closes **1.3**.
9. **Metadata and doc nits:** update the Cargo.toml description (closes **5.5**); align the
   triple-vs-five key wording in both doc sites (closes **7.4**); tolerance-or-caveat for
   `round_robin_balances_clusters` (closes **1.4**); move `Lcg` to `src/lcg.rs` when the crate
   grows, or note the coupling rationale in the module doc (closes **7.3**).

</details>
