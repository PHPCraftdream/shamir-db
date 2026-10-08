<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-bench-utils — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Inline test layout and stale documentation remain, but layout alone is low-severity hygiene, not a runtime High. The unrelated-export objection is refuted by the explicit closely-coupled-group allowance.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 4 | 0 | 0 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Entire test module embedded inline in `vector_data.rs`, violating the mandatory `tests/` layout

Status: `confirmed-open`. Current risk: `low`.

The nine-test inline block violates the rule, but lib.rs and cfg(test) register it correctly. No missing discovery or runtime High mechanism follows from placement.

Evidence: [crates/shamir-bench-utils/src/lib.rs:17](../../../../../crates/shamir-bench-utils/src/lib.rs#L17); [crates/shamir-bench-utils/src/vector_data.rs:217](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L217); [CLAUDE.md:594](../../../../../CLAUDE.md#L594).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `peak_mem.rs` docs still teach Criterion as the module's usage pattern — the harness the workspace removed and banned

Status: `confirmed-open`. Current risk: `low`.

The ignored Criterion recipe and setup wording remain. Actual consumers use Harness, and measure/measure_async/current_allocated still have no executable callers.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:10](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L10); [crates/shamir-bench-utils/src/peak_mem.rs:44](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L44); [crates/shamir-index/benches/create_index_streaming.rs:65](../../../../../crates/shamir-index/benches/create_index_streaming.rs#L65); [CLAUDE.md:237](../../../../../CLAUDE.md#L237).

Grouping/duplicate: `SUMMARY.md#7.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Stale "criterion bench" cross-reference in `vector_data.rs` module doc

Status: `confirmed-open`. Current risk: `low`.

The qualifier still contradicts vector_search's Harness import and migration documentation.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:3](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L3); [crates/shamir-engine/benches/vector_search.rs:46](../../../../../crates/shamir-engine/benches/vector_search.rs#L46); [crates/shamir-engine/benches/vector_search.rs:53](../../../../../crates/shamir-engine/benches/vector_search.rs#L53).

Grouping/duplicate: `SUMMARY.md#7.2`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `vector_data.rs` carries three public exports (`Lcg`, `ClusteredDataset`, `clustered_vectors`) — borderline against one-file-one-export

Status: `refuted`. Current risk: —.

The rule explicitly permits closely coupled groups. The generator directly instantiates Lcg, consumes its Gaussian method, and returns ClusteredDataset; these constitute one fixture-generation unit. A split is optional organization, not a proven violation.

Evidence: [CLAUDE.md:505](../../../../../CLAUDE.md#L505); [crates/shamir-bench-utils/src/vector_data.rs:185](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L185); [crates/shamir-bench-utils/src/vector_data.rs:204](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L204); [crates/shamir-bench-utils/src/vector_data.rs:209](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L209).

Grouping/duplicate: `SUMMARY.md#7.3`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Reproducibility key described inconsistently: "(k, σ, seed) triple" vs. five values

Status: `confirmed-open`. Current risk: `nit`.

The module still tells readers to report only a triple, while function documentation requires five inputs. The consumer lists five values but erroneously calls them a triple.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:29](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L29); [crates/shamir-bench-utils/src/vector_data.rs:158](../../../../../crates/shamir-bench-utils/src/vector_data.rs#L158); [crates/shamir-engine/benches/vector_search.rs:19](../../../../../crates/shamir-engine/benches/vector_search.rs#L19).

Grouping/duplicate: `SUMMARY.md#7.4`. This row is not another independent defect.

## Corrections and qualified non-findings

- Downgrade the original High for inline tests: this is mandatory structural hygiene, not demonstrated behavioral breakage. Stale Criterion documentation is likewise documentation severity.
- lib.rs remains declaration-only; own imports are at module headers except the permitted test use super::*; no current mod.rs contains implementation.
- Named bench/example/test references resolve, but lib.rs also contains an external-checkout harness pointer, so the blanket claim that every documentation pointer resolves within the repository is too broad.
- The historical manifest comment does not disable unit tests or create an authorized layout exception.
- Do not require unrelated implementation splitting as part of moving tests; the documented coupled-group allowance applies.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-bench-utils -- Style & CLAUDE.md structural conformance

## Summary

The crate is structurally minimal (3 source files, no `mod.rs` needed) and scores well on
most of this theme's axes: `lib.rs` is declaration-only (satisfying the re-export-only
spirit), every `use` sits at file top (the sole `use super::*;` is the documented test
exception), `peak_mem.rs` is one closely-coupled group, and the cross-references in doc
comments (`hnsw_rs_contract_tests::lcg_vec`, referenced bench/example paths) all resolve
to real files. The two real problems are (1) `vector_data.rs` embeds its entire test
module inline, directly violating the "never embed `#[cfg(test)] mod tests`" layout rule,
and (2) `peak_mem.rs`'s doc comments still teach Criterion (`iter_custom`,
`criterion_main!`) as the module's usage pattern, a harness CLAUDE.md declares removed
workspace-wide and bans reaching for.

## Findings

### 1. Entire test module embedded inline in `vector_data.rs`, violating the mandatory `tests/` layout

- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:217-363`
- **Severity:** high
- **Issue:** CLAUDE.md § "Test organisation" rule 5 is unambiguous: *"**Never embed
  `#[cfg(test)] mod tests { ... }` inline** inside implementation files. Move them to the
  `tests/` directory."* This file carries its full 9-test suite (~150 lines:
  determinism, clustering-shape, clamping, LCG-stream tests) as an inline `#[cfg(test)]
  mod tests` block — the exact pattern the rule forbids. The crate's `Cargo.toml`
  comment ("Tests live alongside their callers; this crate is a thin helper",
  `Cargo.toml:10`) reads as a self-granted exception, but conventions come from
  CLAUDE.md only, which sanctions no per-crate carve-out; sibling crates
  (`shamir-index/src/vector/tests/*.rs`) follow the mandated layout.
- **Failure scenario:** test files don't get topic-split or discovered the way the
  convention guarantees; `git blame` on implementation vs. tests entangles; the
  deviation normalizes copy-paste of inline tests into other crates.
- **Suggested fix:** convert to `src/vector_data/mod.rs` (module declarations +
  `#[cfg(test)] mod tests;` wire), implementation in a sibling file, and split tests
  per rule 2 into `src/vector_data/tests/{mod.rs, lcg_tests.rs,
  clustered_vectors_tests.rs}` with a re-export-only `tests/mod.rs` manifest, matching
  the documented example layout. Note the same `Cargo.toml` comment conflates
  `doctest = false` with unit-test placement — only the former is a real setting; fix
  the comment while moving the tests.

### 2. `peak_mem.rs` docs still teach Criterion as the module's usage pattern — the harness the workspace removed and banned

- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:10-30` (module doc:
  "# Usage with Criterion `iter_custom`" + full `iter_custom` example),
  `peak_mem.rs:15` (`criterion_main`), `peak_mem.rs:44` ("before `criterion_main!` or
  equivalent")
- **Severity:** medium
- **Issue:** CLAUDE.md is emphatic that the workspace migrated off Criterion on
  2026-07-07: benches use `bench_scale_tool::Harness`, Criterion APIs "no longer apply
  to this repo" and must not be reached for. Yet this module's primary usage
  documentation is a Criterion `iter_custom` recipe. No `criterion` dependency exists
  anywhere under `crates/*/Cargo.toml`, and the module's actual live consumers
  (`crates/shamir-index/benches/create_index_streaming.rs:179-197`,
  `crates/shamir-engine/benches/streaming_topk.rs:114-127`) call
  `setup()`/`reset()`/`current_peak()` directly beside `bench_scale_tool::Harness` —
  the pattern the docs never show. Compounding it, `measure`, `measure_async`, and
  `current_allocated` (`peak_mem.rs:67-110`) — whose only documented use is that
  Criterion example — have zero callers workspace-wide (grep confirms references only
  inside this file's doc comments).
- **Failure scenario:** a contributor adding peak-mem sampling copies the module doc
  example and reintroduces a Criterion dev-dependency, exactly the
  "repeatedly-forgotten convention" CLAUDE.md warns about; the dead
  `measure`/`measure_async` API lingers as untested public surface documented against
  a harness that cannot compile here.
- **Suggested fix:** rewrite the module doc and `setup()` doc around the real
  integration: enable the `peak_mem` feature, call `setup()`, bracket workload with
  `reset()`/`current_peak()` inside a `bench_scale_tool::Harness` bench (copy the
  `create_index_streaming.rs` pattern). Delete or explicitly mark
  `measure`/`measure_async`/`current_allocated` as currently unused (or remove them)
  rather than documenting them via a banned harness.

### 3. Stale "criterion bench" cross-reference in `vector_data.rs` module doc

- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:3`
- **Severity:** low
- **Issue:** The doc says the generator is "Shared between the criterion bench (V0.3,
  `benches/vector_search.rs`) and the recall/RSS report tool". The referenced file
  exists but is no longer a Criterion bench — its own header states "Migrated to the
  fixed-iteration harness (`bench_scale_tool`)"
  (`crates/shamir-engine/benches/vector_search.rs:46-49,53`). Comment discipline here
  is doc-accuracy: the qualifier contradicts both the referenced file and CLAUDE.md's
  bench convention.
- **Failure scenario:** a reader follows the pointer expecting a Criterion integration
  example and finds `bench_scale_tool`, or concludes Criterion is still sanctioned
  somewhere in the workspace.
- **Suggested fix:** drop the "criterion" qualifier — e.g. "shared between the
  vector-search bench (V0.3, `benches/vector_search.rs`, `bench_scale_tool`) and the
  recall/RSS report tool (V0.4, `examples/vector_report.rs`)".

### 4. `vector_data.rs` carries three public exports (`Lcg`, `ClusteredDataset`, `clustered_vectors`) — borderline against one-file-one-export

- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:52` (`pub struct Lcg`),
  `:113` (`pub struct ClusteredDataset`), `:164` (`pub fn clustered_vectors`)
- **Severity:** low
- **Issue:** CLAUDE.md's discipline rule: one file = one primary export, or a
  *closely-coupled* group; multiple *unrelated* public types must be split. The
  borderline call is `Lcg`: it is a fully self-contained, general-purpose RNG value
  type (own doc, own constants, mirrored independently in five
  `shamir-index/src/vector/tests/*` `lcg_vec` helpers), not inherently tied to dataset
  generation; `ClusteredDataset` + `clustered_vectors` are genuinely one coupled unit.
  Grouping all three is defensible for a 2-module crate, but `Lcg` would sit more
  cleanly in its own `lcg.rs` per the rule's letter and its `git blame` rationale.
- **Failure scenario:** none at runtime — this is a structural-hygiene judgment call,
  flagged because the rule is documented as strict.
- **Suggested fix:** if the crate grows at all, move `Lcg` (plus `LCG_MULT`/`LCG_INC`)
  to `src/lcg.rs` and keep `vector_data.rs` to the generator + result type. Acceptable
  to defer while the crate stays this small — but then note the coupling rationale in
  the module doc.

### 5. Reproducibility key described inconsistently: "(k, σ, seed) triple" vs. five values

- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:29` ("The `(k, σ, seed)`
  triple is the reproducibility key") vs. `:158-159` ("Same `(n, dim, k_clusters,
  sigma, seed)` → byte-identical ... Surface these five values")
- **Severity:** nit
- **Issue:** The module-level doc calls a "triple" the reproducibility key while the
  function doc correctly requires all five parameters (output depends on `n` and `dim`
  too, so the triple alone is insufficient). The same triple-vs-five slip appears in
  the downstream bench doc (`vector_search.rs:19`), suggesting the drift propagated
  from here.
- **Failure scenario:** a published report surfaces only `(k, σ, seed)` per the module
  doc and the dataset — hence the recall numbers — is not reproducible.
- **Suggested fix:** align both doc sites on the five-value key `(n, dim, k_clusters,
  σ, seed)`; fix the downstream mention opportunistically in the owning crate.

</details>
