<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-bench-utils — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Coverage gaps, the floating-point endpoint violation, and documentation defects remain. The Gaussian formula is currently correct; the alleged portability flake is unverified. No remediation is source-proven complete.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 8 | 0 | 0 | 0 | 1 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — peak_mem has zero test coverage, unpinned dependency semantics, and dead public API

Status: `confirmed-open`. Current risk: `medium`.

peak_mem has no registered tests; doctests are disabled. measure, measure_async, and current_allocated have no executable workspace callers. Wrapper reset semantics have no regression oracle, although the dependency version itself is locked.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:54](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L54); [crates/shamir-bench-utils/src/peak_mem.rs:85](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L85); [crates/shamir-bench-utils/Cargo.toml:11](../../../../../crates/shamir-bench-utils/Cargo.toml#L11); [Cargo.lock:2396](../../../../../Cargo.lock#L2396).

<a id="review-2"></a>

### Claim 2 — Box-Muller scale is unpinned — a transcription bug in `next_gaussian` would pass the entire suite

Status: `confirmed-open`. Current risk: `medium`.

The correct sqrt(-2*ln(s)/s) formula remains, but registered tests contain neither Gaussian golden values nor moment bounds. Comparing two calls to the same implementation cannot detect stable algorithm drift. The proposed mutation's complete-suite outcome was not executed.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:96](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L96); [crates/shamir-bench-utils/src/vector_data.rs:234](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L234); [crates/shamir-bench-utils/src/vector_data.rs:290](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L290).

<a id="review-3"></a>

### Claim 3 — `Lcg::next_f32` violates its documented `[0, 1)` contract — can return exactly 1.0

Status: `confirmed-open`. Current risk: `low`.

The high-u32-to-f32 conversion can round to 2^32, yielding 1.0 after division. The odd multiplier makes such successor states reachable. No boundary test exists; Gaussian rejection prevents accepting the resulting unit-disk endpoint.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:67](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L67); [crates/shamir-bench-utils/src/vector_data.rs:75](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L75); [crates/shamir-bench-utils/src/vector_data.rs:95](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L95).

<a id="review-4"></a>

### Claim 4 — Stale Criterion-era docs contradict CLAUDE.md's normative bench convention

Status: `confirmed-open`. Current risk: `low`.

The module still demonstrates iter_custom and criterion_main, and vector_data still calls its consumer a Criterion bench. Live consumers import Harness.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:10](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L10); [crates/shamir-bench-utils/src/peak_mem.rs:44](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L44); [crates/shamir-bench-utils/src/vector_data.rs:3](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L3); [CLAUDE.md:237](../../../../../CLAUDE.md#L237).

Grouping/duplicate: `SUMMARY.md#7.2`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — k-clamp asymmetry breaks the "(k, sigma) recoverable from the artefact" claim

Status: `confirmed-open`. Current risk: `low`.

For positive n, requested k is discarded by min(k,n); for n=0, all requested centroids survive. The blanket clamp documentation omits that exception, and the artifact stores neither sigma nor requested parameters.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:111](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L111); [crates/shamir-bench-utils/src/vector_data.rs:153](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L153); [crates/shamir-bench-utils/src/vector_data.rs:179](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L179).

Grouping/duplicate: `SUMMARY.md#5.2`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — Inline `#[cfg(test)] mod tests` violates CLAUDE.md test-organisation rule 5

Status: `confirmed-open`. Current risk: `low`.

All nine tests remain inline, contrary to the explicit layout rule. They are registered through lib.rs and vector_data's cfg(test) block; placement does not prevent discovery.

Evidence: [crates/shamir-bench-utils/src/lib.rs:17](../../../../../crates/shamir-bench-utils/src/lib.rs#L17); [crates/shamir-bench-utils/src/vector_data.rs:217](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L217); [CLAUDE.md:594](../../../../../CLAUDE.md#L594).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — `clustered_vectors` Panics section omits the `dim == 0` assert; `sigma` domain undocumented

Status: `confirmed-open`. Current risk: `nit`.

The Panics section still lists only zero clusters, while zero dimension also asserts. No zero-dimension test or finite/nonnegative sigma contract exists; sigma is multiplied into every generated coordinate without validation.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:163](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L163); [crates/shamir-bench-utils/src/vector_data.rs:172](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L172); [crates/shamir-bench-utils/src/vector_data.rs:204](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L204); [crates/shamir-bench-utils/src/vector_data.rs:341](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L341).

Grouping/duplicate: `SUMMARY.md#6.1`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Cargo.toml description advertises removed functionality

Status: `confirmed-open`. Current risk: `nit`.

The description still advertises BENCH_QUICK, but current exports provide only vector_data and optional peak_mem; lib.rs explicitly records removal of tier tuning.

Evidence: [crates/shamir-bench-utils/Cargo.toml:6](../../../../../crates/shamir-bench-utils/Cargo.toml#L6); [crates/shamir-bench-utils/src/lib.rs:9](../../../../../crates/shamir-bench-utils/src/lib.rs#L9); [crates/shamir-bench-utils/src/lib.rs:14](../../../../../crates/shamir-bench-utils/src/lib.rs#L14).

Grouping/duplicate: `SUMMARY.md#5.5`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — `round_robin_balances_clusters` asserts a statistical property as an exact equality

Status: `unverified`. Current risk: `nit` (provisional; not a confirmed defect).

The test does infer assignments by nearest centroid rather than observing i%k. However, its fixed seed and small sigma may give ample separation; no near-tie bound, affected target, or observed failure establishes the alleged portability flake.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:201](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L201); [crates/shamir-bench-utils/src/vector_data.rs:271](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L271); [crates/shamir-bench-utils/src/vector_data.rs:285](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L285).

## Corrections and qualified non-findings

- peak_mem measures tracked live heap bytes, not OS RSS; vector_report samples RSS separately through memory-stats.
- The manifest's version requirement 0.3 does not automatically admit 0.4, and Cargo.lock pins 0.3.0. The open issue is absent behavioral tests, not an absent version pin.
- The nine inline tests are wired. same_seed_is_byte_identical uses f32 equality, not byte/bit comparisons or historical golden output.
- The Gaussian coverage gap is proven; a current wrong distribution and the claim that a particular mutation passes every test are not experimentally established.
- A 24-bit conversion changes the float stream and needs an explicit compatibility decision. Cross-target exact transcendental golden values need the existing target caveat.
- Moving tests is a layout correction, not a prerequisite for adding missing error-path or peak-memory tests.

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
