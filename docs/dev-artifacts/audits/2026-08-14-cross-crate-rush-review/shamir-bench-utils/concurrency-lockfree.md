<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-bench-utils — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Own-source lock/map guarantees hold. Process-global measurement overlap and documentation gaps remain; the dependency-internal reset race cannot be independently verified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 4 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- The pinned peak_alloc source is available: reset's two-atomic interleaving is confirmed; the separate cancellation carry-over claim remains refuted.

<a id="review-1"></a>

### Claim 1 — `reset()` TOCTOU silently loses concurrent allocations from the peak watermark

Status: `confirmed-open`. Current risk: `low`.

Parent inspection of checksummed peak_alloc 0.3.0 proves reset_peak_usage is PEAK.store(CURRENT.load(Relaxed), Relaxed), while allocation uses CURRENT.fetch_add and PEAK.fetch_max. An allocation between the reset load and store can have its new high watermark overwritten by the older baseline; no later allocation need repair it. This is source-proven measurement correctness, not measured production overhead.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:57](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L57); [Cargo.lock:2396](../../../../../Cargo.lock#L2396).

Pinned dependency evidence: [peak_alloc 0.3.0, src/lib.rs:108](https://docs.rs/crate/peak_alloc/0.3.0/source/src/lib.rs); [peak_alloc 0.3.0, src/lib.rs:125](https://docs.rs/crate/peak_alloc/0.3.0/source/src/lib.rs).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Concurrent `measure`/`measure_async` calls cross-contaminate with no detection

Status: `confirmed-open`. Current risk: `low`.

Both helpers reset and read the same global allocator watermark with no ownership or reentry check. Overlapping resets invalidate another measurement window. Neither helper has an executable workspace caller; live raw-reset samples are sequential.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:40](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L40); [crates/shamir-bench-utils/src/peak_mem.rs:89](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L89); [crates/shamir-bench-utils/src/peak_mem.rs:106](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L106); [crates/shamir-engine/benches/streaming_topk.rs:143](../../../../../crates/shamir-engine/benches/streaming_topk.rs#L143).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Doc asymmetry: `measure` carries none of the concurrency caveats `measure_async` has

Status: `confirmed-open`. Current risk: `nit`.

The synchronous helper still lacks process-global/concurrent-use warnings. The asynchronous warning still incorrectly suggests current_thread alone provides accurate per-task isolation.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:71](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L71); [crates/shamir-bench-utils/src/peak_mem.rs:98](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L98).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `#[global_allocator]` shipped from a library crate

Status: `confirmed-open`. Current risk: `nit`.

The feature-gated library still defines the allocator globally. Linking it affects the entire binary and cannot coexist with another global allocator; the relevant constraint is absent from its module documentation.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:39](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L39); [crates/shamir-bench-utils/src/lib.rs:14](../../../../../crates/shamir-bench-utils/src/lib.rs#L14); [crates/shamir-db/benches/bench_allocator.rs:8](../../../../../crates/shamir-db/benches/bench_allocator.rs#L8).

Grouping/duplicate: `SUMMARY.md#5.1`. This row is not another independent defect.

## Corrections and qualified non-findings

- Own source contains no Mutex/RwLock, concurrent map, hash-keyed structure, or scc len call. Its Vec lengths are constant-time. The dependency's exact atomics/orderings were not verified.
- Calling the crate compliant with all five pillars overlooks the separately confirmed per-point allocation loop.
- A measurement-in-flight guard prevents overlapping cooperating measurements, not arbitrary allocator activity or a dependency-internal reset TOCTOU.
- current_thread does not exclude unrelated tasks, other process threads, or spawn_blocking. Sequential call sites are source-proven; universally sound published peaks are not.
- The duplicate-allocator diagnostic number was not compiler-verified; do not assert E0152/E0159.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-bench-utils -- Concurrency & lock-free invariants

## Summary

The crate is compliant with all five pillars: it contains no `Mutex`/`RwLock`/`parking_lot`, no `scc`/`dashmap`/`ArcSwap`, and no hash-keyed structures at all (pillar 4 is vacuous); the only synchronization primitives are `peak_alloc`'s two `Relaxed` `AtomicUsize` counters (lock-free, O(1) per op), and the only `len()` calls are O(1) `Vec::len()` — no `scc::*::len()` anywhere, so the `clippy.toml` disallowed-methods ban is trivially satisfied. `Lcg` is a pure value type, explicitly documented "no global state, no locking", and dataset generation is single-threaded by design for the determinism contract. All findings below are bench-accuracy issues in `peak_mem`'s process-global peak watermark (a verified non-atomic reset TOCTOU plus doc gaps around concurrent measurement); nothing is memory-unsafe, nothing sits on a hot path, and `peak_mem` is feature-gated off by default. Note `peak_mem` has zero tests, so these invariants are doc-guarded only (the consumers' `current_thread`-runtime pattern is the actual enforcement today).

## Findings

### 1. `reset()` TOCTOU silently loses concurrent allocations from the peak watermark
- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:57-59`
- **Severity:** low
- **Issue:** `reset()` delegates to `PeakAlloc::reset_peak_usage()`, which (verified against peak_alloc 0.3.0 source, `lib.rs:108-110`) is `PEAK.store(CURRENT.load(Relaxed), Relaxed)` — two independent atomic ops, not one CAS. An `alloc` on another thread landing between the load and the store performs `CURRENT.fetch_add` + `PEAK.fetch_max` (`lib.rs:124-128`), and the subsequent store erases that contribution.
- **Failure scenario:** any bench that keeps a background allocating thread alive (multi-threaded runtime, rayon pool, `spawn_blocking`) while calling `reset()` + a measurement under-reports the peak, silently and load-dependently — two runs can disagree. Current consumers avoid this only by convention (both build a `new_current_thread` runtime and drive exactly one workload between reset and read: `crates/shamir-index/benches/create_index_streaming.rs:165-193`, `crates/shamir-engine/benches/streaming_topk.rs:113-133`), and neither `reset`'s nor `measure`'s doc states that requirement.
- **Suggested fix:** document the serial-measurement contract on `reset`/`measure`/`current_peak` (naming the `current_thread`-runtime pattern), and/or add an `AtomicBool` "measurement in flight" guard in `reset()` (`compare_exchange` that panics/logs on re-entry) so concurrent or nested measurement fails loudly instead of quietly.

### 2. Concurrent `measure`/`measure_async` calls cross-contaminate with no detection
- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:85-110`
- **Severity:** low
- **Issue:** the measurement window (reset → run → read) operates on one process-global watermark. Two overlapping calls — two tasks on a multi-threaded runtime, or `measure_async` interleaved with any other `reset()` — destroy each other's baseline: the second `reset()` erases the first's, and both `current_peak()` reads return a merged maximum. `measure_async`'s doc (`:95-101`) covers foreign-task pollution on a multi-threaded executor but not the overlapping-measurements case; nothing detects either.
- **Failure scenario:** dormant API today — no workspace caller of `measure`/`measure_async` exists (both consumers call raw `reset()`/`current_peak()`), so the first future adopter on a multi-threaded runtime gets quietly wrong numbers and can draw a wrong /opti baseline-vs-after conclusion.
- **Suggested fix:** the same in-flight `AtomicBool` guard as finding 1 covers this too (one mechanism, both hazards); minimally, scope the doc to "one measurement at a time, process-wide; run on a `current_thread` runtime".

### 3. Doc asymmetry: `measure` carries none of the concurrency caveats `measure_async` has
- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:71-93` (vs `:95-101`; module example at `:19-29`)
- **Severity:** nit
- **Issue:** `measure` has the identical global-counter hazards as `measure_async` (foreign-thread allocations inflating the peak; concurrent `reset()`), but its docs are silent, while `measure_async` documents the multi-threaded-executor caveat. The module-level usage example demonstrates `measure` inside an async bench loop (`to_async(&rt)`) — exactly the shape where the caveat applies.
- **Failure scenario:** a contributor copies the module example onto a multi-threaded runtime and trusts the returned peak.
- **Suggested fix:** hoist one concurrency section into the module docs covering `setup`/`reset`/`measure`/`measure_async`/`current_peak` uniformly: process-global counters, serial measurement only, `current_thread` runtime per measurement (link `create_index_streaming.rs` / `streaming_topk.rs` as the reference pattern).

### 4. `#[global_allocator]` shipped from a library crate
- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:39-40`
- **Severity:** nit
- **Issue:** enabling the off-by-default `peak_mem` feature (already done by dev-deps in `crates/shamir-index/Cargo.toml:64` and `crates/shamir-engine/Cargo.toml:107`) installs `PeakAlloc` as the process allocator for every final binary that links this crate, and conflicts at compile time (duplicate `#[global_allocator]`, E0152) with any consumer binary defining its own — e.g. the workspace's allocator switch `crates/shamir-db/benches/bench_allocator.rs:8-25` (sefer/mimalloc). Today the conflict is dodged only by convention, noted in each consumer (`create_index_streaming.rs:24`) rather than where the allocator is defined.
- **Failure scenario:** a future bench combining the `bench_allocator.rs` include! switch with peak-RSS sampling fails to link (loud); more subtly, feature unification could enable the allocator for an unintended binary in the dev-dep graph and perturb its allocation profile.
- **Suggested fix:** state the constraint in `peak_mem`'s module docs — "installs the process allocator for any binary that links this crate with the feature on; never combine with another `#[global_allocator]`" — so the allocator definition is the single source of truth instead of per-consumer NOTES.

</details>
