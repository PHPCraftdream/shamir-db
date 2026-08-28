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
