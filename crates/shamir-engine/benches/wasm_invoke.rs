//! Criterion benchmarks for WASM function invocation paths.
//!
//! Three groups:
//!
//! 1. **`wasm_cold_first_call`** — compile + first instantiate + call (cold path).
//! 2. **`wasm_hot_repeat_call`** — reuse a pre-compiled `WasmFunction`, measure
//!    only the per-call cost (instantiate + invoke without recompilation).
//!    This is the primary metric for the InstancePre optimisation (Phase 1).
//! 3. **`wasm_startup_compile_k`** — compile K functions from WAT to measure
//!    load-on-open cost. Primary metric for AOT `.cwasm` cache (Phase 2).
//!
//! Respects `BENCH_QUICK=1` for fast feedback (sample_size=10, 1 s
//! measurement_time, 100 ms warm-up).

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

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

// ── Helpers ──────────────────────────────────────────────────────────

fn quick() -> bool {
    std::env::var_os("BENCH_QUICK").is_some()
}

fn quick_aware_criterion() -> Criterion {
    let c = Criterion::default();
    if quick() {
        c.sample_size(10)
            .measurement_time(Duration::from_secs(1))
            .warm_up_time(Duration::from_millis(100))
    } else {
        c
    }
}

fn build_params() -> Params {
    let mut p = Params::new();
    p.set("x", QueryValue::Int(42));
    p.set("name", QueryValue::Str("hello".into()));
    p
}

// ── Group 1: cold first call (compile + instantiate + call) ──────────

fn bench_cold_first_call(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let params = build_params();
    let ctx = FnCtx::new();
    let batch = FnBatch::new();

    let mut group = c.benchmark_group("wasm_cold_first_call");
    if quick() {
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(1));
    }

    group.bench_function("identity_compile_and_call", |b| {
        b.to_async(&rt).iter(|| {
            let params = params.clone();
            let ctx = ctx.clone();
            let batch = batch.clone();
            async move {
                let engine = Arc::new(WasmEngine::new().unwrap());
                let wf =
                    WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap();
                let result = wf.call(&ctx, &batch, &params).await.unwrap();
                black_box(result);
            }
        });
    });

    group.finish();
}

// ── Group 2: hot repeat call (pre-compiled, measure call only) ───────

fn bench_hot_repeat_call(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let engine = Arc::new(WasmEngine::new().unwrap());
    let wf = Arc::new(WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap());
    let params = build_params();
    let ctx = FnCtx::new();
    let batch = FnBatch::new();

    let mut group = c.benchmark_group("wasm_hot_repeat_call");
    if quick() {
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(1));
    }

    group.bench_function("identity_call_only", |b| {
        b.to_async(&rt).iter(|| {
            let wf = wf.clone();
            let params = params.clone();
            let ctx = ctx.clone();
            let batch = batch.clone();
            async move {
                let result = wf.call(&ctx, &batch, &params).await.unwrap();
                black_box(result);
            }
        });
    });

    group.finish();
}

// ── Group 3: startup compile K modules (load-on-open) ────────────────

fn bench_startup_compile_k(c: &mut Criterion) {
    let sizes: &[usize] = if quick() { &[10] } else { &[10, 50] };

    let mut group = c.benchmark_group("wasm_startup_compile_k");
    if quick() {
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(1));
    }

    for &k in sizes {
        group.bench_with_input(BenchmarkId::from_parameter(k), &k, |b, &k| {
            b.iter(|| {
                let engine = Arc::new(WasmEngine::new().unwrap());
                for _ in 0..k {
                    let wf =
                        WasmFunction::from_wat(engine.clone(), IDENTITY_WAT, WasmLimits::default())
                            .unwrap();
                    black_box(&wf);
                }
            });
        });
    }

    group.finish();
}

// ── Driver ───────────────────────────────────────────────────────────

criterion_group! {
    name = benches;
    config = quick_aware_criterion();
    targets =
        bench_cold_first_call,
        bench_hot_repeat_call,
        bench_startup_compile_k
}
criterion_main!(benches);
