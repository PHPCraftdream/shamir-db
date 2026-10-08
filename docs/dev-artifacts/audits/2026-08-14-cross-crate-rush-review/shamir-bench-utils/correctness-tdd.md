<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-bench-utils — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The nine registered tests exercise the canonical generator, but not independent Gaussian scale, historical bytes, endpoint bounds, dimension rejection, or peak-memory semantics. The endpoint defect is established; the particular alleged portability flake remains unverified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 8 | 0 | 0 | 0 | 1 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — peak_mem has zero test coverage, unpinned dependency semantics, and dead public API

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No module test registration or enabled doctest exercises measurement semantics. reset and current_peak have real bench callers, while measure, measure_async, and current_allocated are unused workspace APIs. Dependency identity is pinned; testing must distinguish reset from stale-peak retention and retain observable allocations during capture.

Evidence: [crates/shamir-bench-utils/src/lib.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/lib.rs#L14); [crates/shamir-bench-utils/src/peak_mem.rs:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L57); [crates/shamir-bench-utils/Cargo.toml:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/Cargo.toml#L11); [Cargo.lock:2396](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2396).

<a id="review-2"></a>

### Claim 2 — Box-Muller scale is unpinned — a transcription bug in `next_gaussian` would pass the entire suite

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The production formula is correct. Symmetric regeneration cannot detect deterministic drift, and the clustering sanity inequality supplies no lower variance bound. An approved reference stream plus qualified moment bounds would discriminate scale changes; exact cross-target Gaussian goldens and a complete-suite mutation result are not established.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L96); [crates/shamir-bench-utils/src/vector_data.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L234); [crates/shamir-bench-utils/src/vector_data.rs:326](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L326).

<a id="review-3"></a>

### Claim 3 — `Lcg::next_f32` violates its documented `[0, 1)` contract — can return exactly 1.0

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The upper 128 possible high words round to 2^32. Multiplication by the odd constant is invertible modulo 2^64, so public seed selection can produce an endpoint on the first call. No registered boundary assertion targets it.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:42](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L42); [crates/shamir-bench-utils/src/vector_data.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L67); [crates/shamir-bench-utils/src/vector_data.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L75).

<a id="review-4"></a>

### Claim 4 — Stale Criterion-era docs contradict CLAUDE.md's normative bench convention

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Module examples and cross-references retain Criterion language despite the explicit Harness convention and actual Harness consumers. This is stale integration guidance, not a failing current execution proven by this audit.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L10); [crates/shamir-bench-utils/src/vector_data.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L3); [CLAUDE.md:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L237).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — k-clamp asymmetry breaks the "(k, sigma) recoverable from the artefact" claim

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Positive n clamps k while n=0 deliberately retains all centroids, contradicting the blanket function wording. Sigma is absent and cannot be recovered generally. This does not make effective k wrong or invalidate determinism from retained inputs.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L111); [crates/shamir-bench-utils/src/vector_data.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L153); [crates/shamir-bench-utils/src/vector_data.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L179).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — Inline `#[cfg(test)] mod tests` violates CLAUDE.md test-organisation rule 5

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Nine tests remain in the inline block. lib.rs and cfg(test) wire them correctly; moving them is policy remediation and not a prerequisite for adding an oracle.

Evidence: [crates/shamir-bench-utils/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/lib.rs#L17); [crates/shamir-bench-utils/src/vector_data.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L217); [CLAUDE.md:594](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L594).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — `clustered_vectors` Panics section omits the `dim == 0` assert; `sigma` domain undocumented

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The Panics section omits dim=0 and only the zero-cluster rejection has a dedicated test. Sigma multiplies every noise coordinate without a documented finite/nonnegative domain. Nonfinite inputs may poison fixtures; negative sigma's interpretation requires a contract decision rather than assuming production insecurity.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L163); [crates/shamir-bench-utils/src/vector_data.rs:172](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L172); [crates/shamir-bench-utils/src/vector_data.rs:204](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L204); [crates/shamir-bench-utils/src/vector_data.rs:342](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L342).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Cargo.toml description advertises removed functionality

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The description still names BENCH_QUICK while current lib.rs explicitly documents tier API removal and exports no tier functions.

Evidence: [crates/shamir-bench-utils/Cargo.toml:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/Cargo.toml#L6); [crates/shamir-bench-utils/src/lib.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/lib.rs#L9).

Grouping/duplicate: [api-wire-protocol.md#7](api-wire-protocol.md#review-7). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — `round_robin_balances_clusters` asserts a statistical property as an exact equality

Status: `unverified`. Current risk: `nit`.

Prior-cycle decision: `unverified`.

Nearest-centroid counts are not a direct construction-label oracle; equal totals can even hide exchanged assignments. Nonetheless this deterministic seed may be well separated. No near-tie analysis or affected target proves a flake, so replacing equality with arbitrary tolerance is unjustified.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:201](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L201); [crates/shamir-bench-utils/src/vector_data.rs:271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L271); [crates/shamir-bench-utils/src/vector_data.rs:280](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L280); [crates/shamir-bench-utils/src/vector_data.rs:285](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L285).

## Evidence and recipe corrections

- Absent tests do not make the module unavailable for inspection or prove that no code ever compiles or executes; registered memory benches call its reset/read seam.
- A peak&gt;=1 MiB check can pass with stale historical peak or unrelated allocations. An isolated reset-to-current and high-then-release-then-reset oracle is needed.
- Use observable retained allocations or appropriate optimization barriers; a temporary Vec whose only result is its length does not independently prove allocator activity.
- same_seed_is_byte_identical compares f32 values, not to_bits or a historical golden. Two identical faulty implementations satisfy it.
- Nearest-centroid count equality does not require every point to retain its original label; compensating misassignments can preserve counts.
- The upper-end probability is 128/2^32 under uniformly distributed high words, not one in four billion. This is a mathematical boundary fact, not a measured event rate.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-bench-utils -- Correctness & TDD-coverage

## Summary

The crate is two small modules: `vector_data` (seeded LCG + clustered fixture generator, decently tested) and the feature-gated `peak_mem` (global-allocator sampling, tested by nothing). The one genuine logic bug found is a boundary violation in `Lcg::next_f32` (documented `[0, 1)`, can return exactly `1.0`); the more damaging theme finding is vacuous coverage: the Box-Muller scale factor is not pinned by any test, so a classic transcription bug in `next_gaussian` would pass the whole suite while silently mislabelling `sigma` in every published vector-bench number, and `peak_mem`'s reset/peak semantics (which published "PEAK HEAP" figures depend on) rest entirely on untested `peak_alloc 0.3` behavior. Stale Criterion-era docs and a stale `BENCH_QUICK` crate description contradict the workspace's post-2026-07-07 bench conventions.

## Findings

### 1. peak_mem has zero test coverage, unpinned dependency semantics, and dead public API
- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:1-111`; `crates/shamir-bench-utils/Cargo.toml:11`
- **Severity:** medium
- **Issue:** The whole module is unverifiable: no `#[cfg(test)]` anywhere, no `tests/` dir, `doctest = false` in Cargo.toml, and every doc example is `rust,ignore` — nothing that exercises this code is ever compiled or run. Meanwhile its measurement contract is load-bearing: `reset()`'s documented behavior ("reset the peak counter to the current allocation level") is `peak_alloc::PeakAlloc::reset_peak_usage()`'s semantics, and `crates/shamir-index/benches/create_index_streaming.rs:172-176` explicitly builds its reported figure on it ("the raw figure includes the [data_store] baseline"). A `peak_alloc` 0.3 → 0.4 semantics change (e.g. reset-to-zero) would silently flip the meaning of published peak-heap numbers. Also, half the public API is dead: `measure`, `measure_async`, and `current_allocated` have zero callers workspace-wide (grep: only `setup`/`reset`/`current_peak` are used, by two benches), so they drift untested by construction. Against CLAUDE.md's Red/Green/Refactor protocol there is no failing-test-first path for any change here.
- **Failure scenario:** dependency upgrade changes `reset_peak_usage` semantics → bench "PEAK HEAP" figures change meaning with no test failure.
- **Suggested fix:** add `src/peak_mem/tests/` (per CLAUDE.md layout) gated `#[cfg(all(test, feature = "peak_mem"))]`: measure a `vec![0u8; 1 << 20]` closure and assert `peak >= 1 MiB`; assert reset-to-current explicitly (allocate a known block, `reset()`, `current_peak() >= current_usage()`); assert `measure`/`measure_async` return the closure/future's result. Delete or test the dead API.

### 2. Box-Muller scale is unpinned — a transcription bug in `next_gaussian` would pass the entire suite
- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:89-103` (vs tests `:217-363`)
- **Severity:** medium
- **Issue:** The crate's core distribution primitive has no golden-value and no statistical-moment test. Every existing test survives, e.g., dropping the `/s` term (`sqrt(-2 ln s)` instead of `sqrt(-2 ln s / s)`, shrinking the effective sigma by roughly 0.7x at s = 0.5): `points_are_clustered_not_scattered` (`intra < 0.25 * inter`) and `round_robin_balances_clusters` both still pass. Since `(k, sigma, seed)` is the documented reproducibility key that "surfaces in every report" (`vector_data.rs:29-30, 158-159`), a silently wrong scale mislabels every vector bench and recall/RSS report built on this shared fixture — exactly the cross-tool comparability the module exists to guarantee.
- **Failure scenario:** refactor of `next_gaussian` alters the scale factor; all tests stay green; published recall-vs-sigma numbers become wrong relative to earlier reports.
- **Suggested fix:** add a fixed-seed golden test (first N `next_gaussian` values, exact `f32` equality) plus a coarse moment test (|mean| < 0.05 and std within ~5% of 1 over >= 50k draws).

### 3. `Lcg::next_f32` violates its documented `[0, 1)` contract — can return exactly 1.0
- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:71-76` (contract `:71`; downstream `:78-82, 92-95`)
- **Severity:** low
- **Issue:** `(high as f32) / (1u64 << 32) as f32` rounds `u32 -> f32` to nearest; every `high` in `[4294967168, 4294967295]` (128 of 2^32 values, ~3e-8 per draw) rounds up to `2^32`, so the ratio is exactly `1.0`. This breaks `next_range`'s `[lo, hi)` doc (a centroid coordinate can be exactly `+1.0`) and the `[0, 1)` claim. `next_gaussian` happens to stay correct only because the `s < 1.0` acceptance check rejects `s = 1.0 + u2^2` — an undocumented, load-bearing accident. No test asserts the bound, and a naive sweep test would not catch it (needs ~3e7 draws); determinism itself is unaffected.
- **Failure scenario:** negligible for bench numbers, but the contract break is real and invisible to the suite.
- **Suggested fix:** `(high >> 8) as f32 / (1u64 << 24) as f32` (exact division, `0.0 <= v < 1.0` by construction), and add a boundary test that crafts a seed whose next high-32 word is `0xFFFF_FFFF`.

### 4. Stale Criterion-era docs contradict CLAUDE.md's normative bench convention
- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:10-15, 18-19, 44`; `crates/shamir-bench-utils/src/vector_data.rs:3`
- **Severity:** low
- **Issue:** `peak_mem`'s module doc is titled "Usage with Criterion `iter_custom`" and teaches `b.to_async(&rt).iter_custom(...)` / "before `criterion_main!`"; `setup()`'s doc says the same. CLAUDE.md (2026-07-07 migration) mandates `bench_scale_tool::Harness` and says "do NOT reach for Criterion APIs from memory/training data", and this crate's own `lib.rs:9-12` records the Criterion API removal. `vector_data.rs:3` also still calls `benches/vector_search.rs` "the criterion bench". A bench author copying these docs re-introduces the removed harness. (The two real consumers, `create_index_streaming.rs` and `streaming_topk.rs`, already use the correct pattern.)
- **Suggested fix:** rewrite the `peak_mem` usage example against `bench_scale_tool::Harness` (capture `reset()`/`current_peak()` around the bench body, as the live benches do); drop the word "criterion" from `vector_data.rs`.

### 5. k-clamp asymmetry breaks the "(k, sigma) recoverable from the artefact" claim
- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:110-111` (claim) vs `:153-154, 179-183` (behavior); test `:332-339`
- **Severity:** low
- **Issue:** `ClusteredDataset`'s doc says centroids are returned "so the `(k, sigma)` parameters are recoverable from the artefact alone". For `n > 0, k_clusters > n`, `k_eff = min(k_clusters, n)` discards the requested `k` — `k()` returns the clamped value, so the artefact self-description fails exactly in the clamped regime. The asymmetry is undocumented at the function level: the doc's blanket "`k_clusters > n` clamps to `n`" (`:153-154`) does not carve out the `n == 0` path, which deliberately preserves all `k_clusters` (`:176-183`). The `k_greater_than_n_clamps_silently` test codifies the loss (`assert!(ds.k() <= 3)`) without pinning whether `k` is recoverable. Determinism given inputs is unaffected; only artefact self-description.
- **Suggested fix:** at minimum correct both doc sites; better, store the requested `k_clusters` (and `sigma`, `seed`) on the struct so reports truly can surface the key from the artefact.

### 6. Inline `#[cfg(test)] mod tests` violates CLAUDE.md test-organisation rule 5
- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:217`
- **Severity:** low (cross-lens: likely also the style reviewer's; noted here because it is this crate's only test locus)
- **Issue:** CLAUDE.md: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." `vector_data.rs` embeds its nine tests inline. The `Cargo.toml:10` comment ("Tests live alongside their callers; this crate is a thin helper") reads as a deliberate deviation, but no carve-out for small crates exists in the documented rule.
- **Suggested fix:** move to `src/vector_data/tests/` with a manifest-only `mod.rs` per the mandated layout — or, if the deviation is intended, record it as an explicit workspace exception.

### 7. `clustered_vectors` Panics section omits the `dim == 0` assert; `sigma` domain undocumented
- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:161-163` vs `:172`
- **Severity:** nit
- **Issue:** The `# Panics` section documents only `k_clusters == 0`, but `assert!(dim > 0)` also panics (even though `dim == 0` would otherwise work mechanically — empty vectors). `sigma` is unvalidated: negative sigma mirrors the noise (harmless but undocumented), NaN sigma poisons the whole dataset silently. Only the happy path and the one documented panic are tested (`zero_clusters_panics`); there is no `should_panic` test for `dim == 0`.
- **Suggested fix:** document both panics and sigma's expected domain; add the missing `should_panic` test.

### 8. Cargo.toml description advertises removed functionality
- **File:line:** `crates/shamir-bench-utils/Cargo.toml:6`
- **Severity:** nit
- **Issue:** The description says "BENCH_QUICK env-var support for /opti baseline/after pairs", but `BENCH_QUICK` appears nowhere in the crate or any workspace source (only in historical perf-journal/roadmap docs) — it belonged to the tier-tuning era `lib.rs:9-12` says was removed.
- **Suggested fix:** update the description, e.g. "shared clustered-vector fixture generation and optional peak-RSS sampling for workspace benches".

### 9. `round_robin_balances_clusters` asserts a statistical property as an exact equality
- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:267-287`
- **Severity:** nit
- **Issue:** The test requires every point's nearest centroid to equal its generating cluster (otherwise `counts != n / k`). The module doc itself (`:27-29`) says cross-target `f32` `ln`/`sqrt` identity is "not promised", so a near-tie in inter-centroid distances could flip one point's assignment on a different target and fail the exact assertion. Deterministic on a fixed target; brittleness is theoretical, but the test silently conflates "round-robin assigns balanced" with "nearest-centroid recovers the assignment".
- **Suggested fix:** assert balance within a tolerance (e.g. every cluster count within 2 of `n / k`), or keep exact and note the cross-target caveat in the test.

</details>
