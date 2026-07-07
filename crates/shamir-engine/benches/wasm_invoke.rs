// Single-element `for` loops are intentional: the N/K-ladders were
// collapsed to their smallest variant when migrating to the fixed-iteration
// harness, but the loop structure is kept so the ladder can be re-expanded
// ad-hoc.
#![allow(clippy::single_element_loop)]
//! Benchmarks for WASM function invocation paths.
//!
//! Five groups:
//!
//! 1. **`wasm_cold_first_call`** — compile + first instantiate + call (cold path).
//! 2. **`wasm_hot_repeat_call`** — reuse a pre-compiled `WasmFunction`, measure
//!    only the per-call cost (instantiate + invoke without recompilation).
//!    This is the primary metric for the InstancePre optimisation (Phase 1).
//! 3. **`wasm_startup_compile_k`** — compile K functions from WAT to measure
//!    load-on-open cost. Primary metric for AOT `.cwasm` cache (Phase 2).
//! 4. **`wasm_compile_cached`** — cold vs warm disk-cache compile cost.
//! 5. **`wasm_concurrent_calls`** — N parallel calls sharing one pre-compiled
//!    `WasmFunction`, exercising the allocator under concurrency.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`):
//! - Groups 1, 3, 4 rebuild fresh state every call (a fresh `WasmEngine`
//!   must be compiled per iteration — sharing one would just measure the
//!   SECOND+ compile, not the cold path) → `bench_batched_async` /
//!   `bench_batched` (group 3/4's cold-cache and startup-compile variants
//!   are sync, no `.await` in the timed portion beyond a plain compile call
//!   which is itself sync in this API — kept async via `bench_batched_async`
//!   for `wasm_cold_first_call` because `WasmFunction::call` is async).
//! - Group 2 shares ONE pre-compiled `WasmFunction` across iterations
//!   (never invalidated by a call) → `bench_async`.
//! - Group 5 shares ONE pre-compiled `WasmFunction` across iterations,
//!   spawning N fresh tasks each call → `bench_async`.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::function::{
    FnBatch, FnCtx, Params, ShamirFunction, WasmEngine, WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;

// ── WAT: identity/echo module (from unit tests) ─────────────────────
//
// Exports memory, shamir_alloc (bump allocator), shamir_call (echoes
// back the same [ptr, len) region — identity function).

const IDENTITY_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl
        (i64.extend_i32_u (local.get $ptr))
        (i64.const 32)
      )
      (i64.extend_i32_u (local.get $len))
    )
  )
)
"#;

fn build_params() -> Params {
    let mut p = Params::new();
    p.set("x", QueryValue::Int(42));
    p.set("name", QueryValue::Str("hello".into()));
    p
}

fn main() {
    let mut h = Harness::new("wasm_invoke", env!("CARGO_MANIFEST_DIR"));

    // ── Group 1: cold first call (compile + instantiate + call) ──────────
    {
        let params = build_params();
        let ctx = FnCtx::new();
        let batch = FnBatch::new();
        h.bench_batched_async(
            "wasm_cold_first_call/identity_compile_and_call",
            {
                let params = params.clone();
                let ctx = ctx.clone();
                let batch = batch.clone();
                move || {
                    let params = params.clone();
                    let ctx = ctx.clone();
                    let batch = batch.clone();
                    async move { (params, ctx, batch) }
                }
            },
            move |(params, ctx, batch)| async move {
                let engine = Arc::new(WasmEngine::new().unwrap());
                let wf =
                    WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap();
                let result = wf.call(&ctx, &batch, &params).await.unwrap();
                std::hint::black_box(result);
            },
        );
    }

    // ── Group 2: hot repeat call (pre-compiled, measure call only) ───────
    {
        let engine = Arc::new(WasmEngine::new().unwrap());
        let wf =
            Arc::new(WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap());
        let params = build_params();
        let ctx = FnCtx::new();
        let batch = FnBatch::new();

        h.bench_async("wasm_hot_repeat_call/identity_call_only", move || {
            let wf = wf.clone();
            let params = params.clone();
            let ctx = ctx.clone();
            let batch = batch.clone();
            async move {
                let result = wf.call(&ctx, &batch, &params).await.unwrap();
                std::hint::black_box(result);
            }
        });
    }

    // ── Group 3: startup compile K modules (load-on-open) ────────────────
    // Scaled ladder collapsed to k=5 (was `[10, 50]`): each WAT→module
    // compile is a fixed ~1.2ms VM cost, so the total scales linearly with
    // K. k=10 was ~12ms/call; k=5 keeps it under the ≤10ms budget.
    for &k in &[5usize] {
        h.bench(&format!("wasm_startup_compile_k/{k}"), move || {
            let engine = Arc::new(WasmEngine::new().unwrap());
            for _ in 0..k {
                let wf =
                    WasmFunction::from_wat(engine.clone(), IDENTITY_WAT, WasmLimits::default())
                        .unwrap();
                std::hint::black_box(&wf);
            }
        });
    }

    // ── Group 4: cached compile (AOT disk cache) ───────────────────────
    //
    // Cold cache: first compilation populates the cache entry (a fresh
    // engine is required per iteration — sharing one would measure
    // subsequent warm compiles, not the cold path).
    h.bench_batched(
        "wasm_compile_cached/cold_cache",
        || Arc::new(WasmEngine::new().unwrap()),
        |engine| {
            let wf = WasmFunction::from_wat(engine.clone(), IDENTITY_WAT, WasmLimits::default())
                .unwrap();
            std::hint::black_box(&wf);
        },
    );

    // Warm cache: the first compile (in setup, untimed) seeds the disk
    // cache; the timed compile call should hit it. A fresh engine per
    // iteration matches the original per-`b.iter` engine + warm seed.
    h.bench_batched(
        "wasm_compile_cached/warm_cache",
        || {
            let engine = Arc::new(WasmEngine::new().unwrap());
            let _warm = WasmFunction::from_wat(engine.clone(), IDENTITY_WAT, WasmLimits::default())
                .unwrap();
            engine
        },
        |engine| {
            let wf = WasmFunction::from_wat(engine.clone(), IDENTITY_WAT, WasmLimits::default())
                .unwrap();
            std::hint::black_box(&wf);
        },
    );

    // ── Group 5: concurrent calls (pooling vs on-demand) ────────────────
    //
    // Shares a single pre-compiled `Arc<WasmFunction>` across N parallel
    // async calls via `tokio::spawn`. Toggle allocator via
    // `SHAMIR_WASM_NO_POOL=1`.
    {
        let engine = Arc::new(WasmEngine::new().unwrap());
        let wf =
            Arc::new(WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap());
        let params = build_params();

        // Scaled ladder collapsed to n=16 (was `[16, 64, 128]`): each
        // concurrent call instantiates a fresh WASM instance, so cost
        // scales with N. n=16 is ~3.3ms/call; n=128 was ~25ms.
        for &n in &[16usize] {
            let wf = wf.clone();
            let params = params.clone();
            h.bench_async(&format!("wasm_concurrent_calls/{n}"), move || {
                let wf = wf.clone();
                let params = params.clone();
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for _ in 0..n {
                        let wf = wf.clone();
                        let params = params.clone();
                        handles.push(tokio::spawn(async move {
                            let ctx = FnCtx::new();
                            let batch = FnBatch::new();
                            wf.call(&ctx, &batch, &params).await.unwrap()
                        }));
                    }
                    for hd in handles {
                        std::hint::black_box(hd.await.unwrap());
                    }
                }
            });
        }
    }

    h.run();
}
