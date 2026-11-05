# Two-tier WASM compilation: Winch first, Cranelift for hot functions

Status: implemented (core, 2026-11-05; see Implementation notes at the end of this document). Scope: `shamir-wasm-host` (engine and
function slot), call sites in `shamir-db` stay source-compatible. Out of scope here: the
DDL/OQL surface for the mode (PROC-3b), the composite content hash and upload/download
(PROC-1), a new source language (PROC-2).

## Why

A stored procedure is compiled to machine code before its first call. Cranelift produces fast
code but is the slow part of "deploy and call"; the single-pass Winch compiler starts sooner
and its code is slower. Measured on a hosted runner with the real engine configuration
(fuel, epoch interruption, pooling allocator, async host imports), `wasm-bench` run on master
`dfc005ef`:

| | Cranelift | Winch | Pulley (interpreter) |
|---|---:|---:|---:|
| compile, tiny module | 0.75 ms | 0.76 ms | 0.96 ms |
| compile, 64 functions | 9.4 ms | 7.4 ms | 12.3 ms |
| warm call (instantiate + call) | 4.6 us | 5.6 us | 24.7 us |
| compute vs native (loop kernels) | 1.3-1.6x | 1.8-3.7x | 20-27x |
| compute vs native (recursion) | 4.0x | 6.1x | 80x |

Other measured facts: fuel costs about 23% on call-heavy recursion and nothing visible on
loops; epoch costs about 4%; a sync host call costs about 100 ns, an async one about 500 ns;
a database call from a guest adds about 6 us over the same call without a guest.

Consequences: Pulley is not a fast tier (portability fallback only). Winch is a sensible
fast tier: it works with everything the engine configures. Its win is concentrated in large
modules, so the upgrade must be driven by hotness, not done for every function.

## Model

* `WasmEngine` owns two `wasmtime::Engine`s: a **fast** engine (Winch) and an **optimized**
  engine (Cranelift, the current engine, including the incremental compile cache). A `Store`
  and an `InstancePre` must come from the same `Engine`, so a compiled module is always
  carried together with the engine it was compiled for.
* `WasmFunction` keeps its compiled form in an `arc_swap::ArcSwap` slot holding
  `{ engine, instance_pre, tier }`. The call path loads the slot **once**, builds the `Store`
  from that slot's engine and instantiates from that slot's `InstancePre`. A call that started
  on the old slot finishes on it; new calls see the new slot.
* Creation (`from_wat`, `from_binary`, `from_binary_blocking`) runs the structural sanitizer
  exactly as today, then compiles the **fast** tier synchronously and returns.
* A call counter (`AtomicU64`, relaxed) crosses `hot_calls` once; the winning thread
  schedules a single upgrade job: `spawn_blocking` on the shared blocking pool, bounded by a
  semaphore (`background_jobs`), compile with the optimized engine, `instantiate_pre`, swap
  the slot. The job holds a `Weak` to the function state so a dropped function does not keep
  compiling for nobody.
* Modes (`TierMode`): `Auto` (default, as above), `Now` (compile the optimized tier
  synchronously at creation; no fast slot is built and no upgrade is scheduled), `Never` (stay on the fast tier; the upgrade is never
  scheduled).
* Defaults: `hot_calls = 64`, `background_jobs = max(1, cores / 4)`. The break-even point
  depends on the workload: a compute-bound procedure repays the Cranelift compile in a handful
  of calls, a database-bound one in thousands; 64 is a conservative start, configurable in
  `WasmEngineOptions`.

## Failure behaviour

* The fast compile fails (module uses something Winch rejects): compile with the optimized
  engine synchronously, slot starts optimized, log once at `info` naming the function and the
  reason. This is not an error for the caller.
* The background upgrade fails or panics: keep the fast slot, log at `warn`, do not retry in
  a tight loop (one retry on the next threshold crossing at 16x `hot_calls` is acceptable).
* Pooling allocator unavailable for an engine: each engine falls back to the on-demand
  allocator independently, as the single engine does today.

## Resources

* Two engines mean two pooling reservations (virtual address space only). One epoch-ticker
  thread holds both engines' `EngineWeak`s, ticks **both** engines, and exits on the first
  tick after the last strong `Engine` is gone.
* The incremental compile cache is a Cranelift feature: enable it only on the optimized
  engine; the Winch engine compiles without it.
* Binary size: `winch` is a default feature of the `wasmtime` dependency in
  `shamir-wasm-host`, so it is part of the normal build. The `bench-strategies` feature (off
  by default, never enabled by the server build) additionally enables Pulley and the
  `SHAMIR_WASM_STRATEGY` environment switch for A/B runs; it still compiles.

## Security

The sanitizer, size limits and every other gate run before **any** compile, exactly once per
module, shared by both tiers. Fuel, epoch deadlines and memory limits are applied per `Store`
by the existing code path and must behave identically on both tiers. The background compile
is bounded in concurrency; a pathological module can make one pool thread busy, never the
call path.

## Observability

`info` log on every tier decision: function name where known, tier, compile time, module
size. A cheap accessor `WasmFunction::tier() -> Tier` for tests and for the future
`describe_function`. Counters for upgrades started, completed, failed (a plain atomic struct
readable by tests is enough for now).

## Tests (required)

Results identical before and after the swap; the slot becomes optimized after the threshold
and not before; one upgrade under a race of many concurrent calls; a call that started before
the swap finishes correctly; `Never` stays fast; `Now` starts optimized; a failing fast
compile falls back to optimized; a failing upgrade keeps the function working; fuel exhaustion
and epoch interruption trap identically on both tiers; both engines' epochs advance.

## Acceptance

* All existing `shamir-wasm-host` and `shamir-db` function tests pass unchanged except
  where they build `WasmEngineOptions` literally.
* `cargo clippy -p shamir-wasm-host -p shamir-db --all-targets -D warnings` (with and without
  `--features bench-strategies` for the host crate), fmt and `check_layout.py` are clean.
* `wasm-bench` (manual workflow) shows the cold start of a freshly created function on the
  fast tier and the post-upgrade warm call on the optimized tier.

## Implementation notes

Implemented in `shamir-wasm-host` (commit `feat(wasm-host): two-tier compilation core`).
Files:

* `crates/shamir-wasm-host/src/wasm/engine/wasm_engine.rs` — the engine pair, per-tier
  pooling with fallback, the single epoch ticker for both engines, `compile_fast` /
  `compile_optimized`, the upgrade permit semaphore, `TierStats`.
* `crates/shamir-wasm-host/src/wasm/engine/tier.rs` / `tier_mode.rs` / `tier_options.rs` /
  `tier_stats.rs` — `Tier`, `TierMode`, `TierOptions` (defaults: enabled, `hot_calls = 64`,
  `background_jobs = max(1, cores / 4)`), `TierStats`.
* `crates/shamir-wasm-host/src/wasm/engine/tier_upgrade.rs` — `FunctionSlot` /
  `FunctionShared` (the `ArcSwap` slot), the call-path hook `maybe_schedule_upgrade`, the
  bounded background upgrade job with one 16x-threshold retry.
* `crates/shamir-wasm-host/src/wasm/wasm_function.rs` — creation paths (`from_wat`,
  `from_binary`, `from_binary_blocking` and their `_with_mode` variants),
  `compile_initial_slot` (tier decision + `info` log), the public `tier()` accessor.

Deviations from the design as written above:

* `WasmEngine::compile` is test-only: kept under its historical name for the compile-cache
  test suite and compiled under `cfg(test)`; production compiles go through
  `compile_fast` / `compile_optimized`.
* The pooling ceiling follows the optimized engine: `pooling_memory_ceiling` reports the
  optimized engine's active ceiling, and the fast engine pools with the same ceiling or not
  at all. If the fast engine's own pool falls back to on-demand allocation, the optimized
  engine's ceiling still drives limit validation: stricter, never looser.
* The fast engine is not built when tiering is disabled (`TierOptions::enabled == false`):
  a single-tier optimized engine.
* `Now` compiles only the optimized tier (the design text above said both tiers).
* `Never` falls back to the optimized tier when there is no fast engine (tiering disabled or
  the fast engine failed to build); not an error for the caller.
