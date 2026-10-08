<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-bench-utils — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Local example inputs reach both rejection asserts. The dimension path is independently reachable through persisted_hnsw despite vector_report's filter. Existing reset-before-work behavior refutes cancellation carry-over, while ownership and coverage gaps remain separate.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 4 | 0 | 0 | 1 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `clustered_vectors` validates arguments with `assert!` instead of `Result`/`thiserror`, and one caller feeds it externally-controlled input

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

env_usize accepts zero. VR_K_CLUSTERS=0 is passed through DatasetParams to the cluster assert; PH_DIM=0 similarly reaches the dimension assert in another example. Authority is local environment control, not database-client permissions or a shipped server request.

Evidence: [crates/shamir-engine/examples/vector_report.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L110); [crates/shamir-engine/examples/vector_report.rs:406](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L406); [crates/shamir-engine/examples/vector_report.rs:212](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L212); [crates/shamir-engine/examples/persisted_hnsw.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/persisted_hnsw.rs#L83); [crates/shamir-engine/examples/persisted_hnsw.rs:363](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/persisted_hnsw.rs#L363); [crates/shamir-bench-utils/src/vector_data.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L171).

<a id="review-2"></a>

### Claim 2 — Second panic path (`dim == 0`) is missing from the `# Panics` doc and has no test

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The function documents only zero clusters and the test block contains only that rejection oracle. vector_report filters zero dimensions, but persisted_hnsw does not; its environment-derived dimension reaches this undocumented panic.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L163); [crates/shamir-bench-utils/src/vector_data.rs:172](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L172); [crates/shamir-bench-utils/src/vector_data.rs:342](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L342); [crates/shamir-engine/examples/vector_report.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/vector_report.rs#L130); [crates/shamir-engine/examples/persisted_hnsw.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/examples/persisted_hnsw.rs#L207).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — `measure` / `measure_async` have no drop-guard: global peak counter is left perturbed on panic or future cancellation; module has zero tests

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Both subsequent helper entry paths reset before executing work. There is no armed ownership state to restore. Surviving allocations and continuing spawned work belong to baseline or contamination concerns, not inherited historical peak. Missing tests remain an independently open coverage row.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L89); [crates/shamir-bench-utils/src/peak_mem.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L106).

Grouping/duplicate: [SUMMARY.md#2.1](SUMMARY.md#review-2-1). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — `peak_mem` feature silently installs a `#[global_allocator]` into every consumer test/bench/example binary; `setup()` misrepresents the activation model

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The library declaration activates independently of setup for actual linked consumers, and both dev manifests enable it. Every-unit-test wording exceeds linkage evidence; no runtime setup operation switches allocators.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:34](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L34); [crates/shamir-bench-utils/src/peak_mem.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L39); [crates/shamir-bench-utils/src/peak_mem.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L48); [crates/shamir-engine/Cargo.toml:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L107); [crates/shamir-index/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L64).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Tests are embedded inline contrary to the documented layout, which is where the error-path gap lives

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The inline block and comment remain, without a sanctioned small-crate exception. The tests are registered; their placement neither causes nor prevents the missing error-path oracle.

Evidence: [crates/shamir-bench-utils/src/vector_data.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/vector_data.rs#L217); [crates/shamir-bench-utils/Cargo.toml:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/Cargo.toml#L10); [CLAUDE.md:594](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L594).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

## Evidence and recipe corrections

- The current vector_report-specific zero-dimension refutation is correct, but it must not become an all-callers assurance: persisted_hnsw accepts PH_DIM=0.
- Changing the helper to Result and adding expect to environment-derived callers does not yield graceful validation; propagate or diagnose those errors at the local tool boundary.
- reset-to-zero conflicts with the documented baseline-preserving contract and exact peak_alloc 0.3.0 implementation.
- There is no present ownership flag requiring cancellation restoration. A future guard needs RAII release, not unconditional global-counter restoration.
- No Result return is not an infallibility proof: allocation and capacity failures remain possible.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-bench-utils -- Error handling & resource lifecycle

## Summary

The crate is tiny and almost entirely infallible-by-construction: no `Result`, `thiserror`, or `anyhow` appears anywhere, and most APIs cannot fail. The one genuinely fallible public API (`clustered_vectors`) validates arguments with `assert!` rather than the CLAUDE.md-mandated `Result<T, E>` + `thiserror` discipline, and one of its two panic paths is both undocumented in `# Panics` and untested. In `peak_mem`, the `measure`/`measure_async` helpers mutate process-global allocator state with no drop-guard for the panic/cancellation path, and the module carries zero tests; the feature-gated `#[global_allocator]` also silently replaces the allocator in every consumer test/bench/example binary that links the crate, a blast radius the docs under-describe.

## Findings

### 1. `clustered_vectors` validates arguments with `assert!` instead of `Result`/`thiserror`, and one caller feeds it externally-controlled input

- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:171-172`
- **Severity:** medium
- **Issue:** The crate's only fallible API signals invalid input via `assert!(k_clusters > 0, ...)` / `assert!(dim > 0, ...)`. CLAUDE.md's error-handling rules require "Return `Result<T, E>`. Avoid `panic!` outside `unreachable!()` / invariant violations" and "`thiserror` for library error enums". One can argue a misconfigured bench fixture is a programmer bug, but the panic is reachable from *user* input, not just code: `shamir-engine/examples/vector_report.rs:406` reads `env_usize("VR_K_CLUSTERS", 64)` and passes the parsed value straight into `clustered_vectors` at line 212. A successfully parsed `0` (env var set to `"0"`) sails past the `unwrap_or(default)` fallback in `env_usize` and detonates the assert deep inside the tool. The library API forces every boundary caller into panic-handling instead of offering a catchable error. (Tempering factors: the panic is documented in `# Panics`, covered by a `#[should_panic]` test, and all bench callers pass literals.)
- **Failure scenario:** `VR_K_CLUSTERS=0 cargo run --release --example vector_report` builds a tokio runtime and starts the report pipeline, then panics mid-run with a bare `"k_clusters must be > 0"` instead of a clean validation message and exit code.
- **Suggested fix:** Change the signature to `-> Result<ClusteredDataset, VectorDataError>` with a small `thiserror` enum (`#[error("k_clusters must be > 0")] ZeroClusters`, same for `ZeroDim`); update the ~8 bench/example call sites to `.expect(...)` at their own boundaries (anyhow/expect is sanctioned in binaries/tests). If the panic stance is deliberately kept for this bench-only helper, it must at least be internally consistent — see finding 2.

### 2. Second panic path (`dim == 0`) is missing from the `# Panics` doc and has no test

- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:172` (assert) vs `:161-163` (`# Panics` doc); test gap at `:341-345`
- **Severity:** medium
- **Issue:** The doc comment promises only "Panics if `k_clusters == 0`", but line 172's `assert!(dim > 0, "dim must be > 0")` is a second, unmentioned panic path. The error-path test suite covers exactly one of the two cases (`zero_clusters_panics`, line 341); there is no `#[should_panic(expected = "dim must be > 0")]` (or `Result`-err) counterpart. This is precisely the "missing error-path tests" gap: a doc-driven caller has no way to know `dim = 0` is fatal.
- **Failure scenario:** A table-driven bench sweep whose dims list is empty or mis-parsed defaults to `dim = 0` and panics on an "impossible" input the documentation said could not panic.
- **Suggested fix:** Whichever way finding 1 lands: either both conditions become `Result` errors (then add a `returns Err for dim == 0` test), or the `# Panics` section documents both paths and a matching `#[should_panic]` test is added for `dim == 0`.

### 3. `measure` / `measure_async` have no drop-guard: global peak counter is left perturbed on panic or future cancellation; module has zero tests

- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:85-93` (`measure`), `:102-110` (`measure_async`)
- **Severity:** low
- **Issue:** Both helpers `reset()` process-global allocator state, run the workload, and capture the peak only on the success path. There is no guard to restore or capture state if `f` panics or — the realistic case for the async variant — the future is dropped mid-`await` (harness timeout / cancellation). After that, the global counter stays "armed" from the stale reset, so the *next* measurement in the same process silently includes the aborted cell's allocations and measures from a wrong baseline. The module also has no tests at all (nothing pins `reset`/`current_peak`/`measure` semantics), so this lifecycle behavior is entirely unpinned.
- **Failure scenario:** A bench harness drops a timed-out `measure_async` future; the following cell's `measure` call reports a peak inflated by the abandoned cell's tail allocations, corrupting a published RSS/peak table without any error signal.
- **Suggested fix:** Introduce a `PeakGuard` (drops to `reset_peak_usage()` or captures-and-restores) held across `f`/`f.await` so the global counter survives unwinding and cancellation; document the cancellation caveat next to the existing multi-thread contamination note; add feature-gated unit tests pinning the reset/capture contract (`measure` returns peak ≥ the closure's allocations; `reset` zeroes the counter).

### 4. `peak_mem` feature silently installs a `#[global_allocator]` into every consumer test/bench/example binary; `setup()` misrepresents the activation model

- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:39-52`; consumers `crates/shamir-engine/Cargo.toml:107`, `crates/shamir-index/Cargo.toml:64`
- **Severity:** low
- **Issue:** Both in-repo consumers enable `features = ["peak_mem"]` in `[dev-dependencies]`. Dev-deps are linked into those crates' unit-test binaries too, so every `./scripts/test.sh` run executes all shamir-engine and shamir-index tests under `PeakAlloc`, not just benches — with no opt-in and no kill switch, because the allocator activates at link time, not when `setup()` is called. The module doc ("off by default so normal `cargo bench` paths are unaffected", lines 3-4) and the `setup()` doc ("Initialize the peak allocator tracking", line 44) both imply an opt-in activation that does not exist; `setup()` is a no-op whose linker comment describes LTO stripping, not activation. Harmless today (peak_alloc adds only atomic counters), but it is an invisible resource-lifecycle side effect: any future behavioral change in peak_alloc propagates into the entire test graph of two core crates. Contrast the repo's own standard: shamir-engine's Cargo.toml carefully documents the blast radius of its `test-util` feature (lines 86-91), while the two `peak_mem` dep lines carry no such comment.
- **Failure scenario:** Mostly latent: a future peak_alloc version changes allocation timing/behavior and engine/index tests start failing with no connection drawn to a "bench-only" feature; or a future crate adds shamir-bench-utils as a *regular* dependency with the feature and pushes `PeakAlloc` into the shipped binary, unnoticed.
- **Suggested fix:** Document the feature-unification consequence in the `peak_mem` module docs (allocator applies at link time to every binary that links the crate, including consumer unit tests) and add a one-line comment to both consumers' Cargo.toml entries; longer term, isolate the `#[global_allocator]` into a bench-target-only path or separate micro-crate so the feature cannot leak into lib test binaries.

### 5. Tests are embedded inline contrary to the documented layout, which is where the error-path gap lives

- **File:line:** `crates/shamir-bench-utils/src/vector_data.rs:217-363`; `crates/shamir-bench-utils/Cargo.toml:10` (`# Tests live alongside their callers; this crate is a thin helper.`)
- **Severity:** nit
- **Issue:** CLAUDE.md "Test organisation" §5 mandates "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." The inline block is institutionalized by the Cargo.toml comment rather than treated as a deviation. The substantive cost is to this theme: there is no `tests/` home for the missing `dim == 0` error-path test (finding 2) or for `peak_mem` coverage (finding 3), and the coverage claim rests on a single `#[should_panic]` case inside an implementation file.
- **Suggested fix:** When findings 2/3 are addressed, move the tests to `src/vector_data/tests/` (+ `src/peak_mem/tests/` under the feature gate) per the documented layout, and drop the Cargo.toml comment that codifies the exception.

</details>
