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
//! 4. **`compile_cold/fn64`, `compile_cold/fn256`, `compile_warm/fn64`,
//!    `compile_warm/fn256`** — cached disk compiles on generated modules.
//!    Cold uses an empty cache per iteration; warm seeds it in untimed setup.
//! 5. **`wasm_concurrent_calls`** — N parallel calls sharing one pre-compiled
//!    `WasmFunction`, exercising the allocator under concurrency.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`):
//! - Groups 1 and 3 build fresh state per iteration. Group 4 keeps compiled
//!   output alive until the next untimed setup, then drops it outside timing.
//! - Group 2 shares ONE pre-compiled `WasmFunction` across iterations
//!   (never invalidated by a call) → `bench_async`.
//! - Group 5 shares ONE pre-compiled `WasmFunction` across iterations,
//!   spawning N fresh tasks each call → `bench_async`.

use std::cell::RefCell;
use std::rc::Rc;
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

fn has_cache_entry_files(path: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_file() && child.extension().is_some_and(|ext| ext == "e") {
            return true;
        }
        if child.is_dir() && has_cache_entry_files(&child) {
            return true;
        }
    }
    false
}

fn module_with_functions(n: usize) -> String {
    assert!(n >= 2, "function count includes the two ABI exports");
    let mut wat = String::from(
        r#"(module
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
"#,
    );
    for i in 0..(n - 2) {
        let constant = i32::try_from(i).expect("function count fits wasm i32");
        wat.push_str(&format!(
            "\n  (func (export \"f{i}\") (param i32) (result i32) (i32.add (local.get 0) (i32.const {constant})))"
        ));
    }
    wat.push_str(
        "
)",
    );
    wat
}

fn build_params() -> Params {
    let mut p = Params::new();
    p.set("x", QueryValue::Int(42));
    p.set("name", QueryValue::Str("hello".into()));
    p
}

fn main() {
    let mut h = Harness::new("wasm_invoke", env!("CARGO_MANIFEST_DIR"));
    let cold_outputs = Rc::new(RefCell::new(None));
    let warm_outputs = Rc::new(RefCell::new(None));

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
    // Measures an UNCACHED compile on purpose: `WasmEngine::new()` has no
    // disk cache, so this is the raw per-module compile baseline.
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

    // ── Group 4: disk-cache compile, cold vs warm ──────────────────────
    // N is the total function count, including the two ABI exports.
    for &n in &[64usize, 256usize] {
        let wat = Arc::new(module_with_functions(n));
        let cold_outputs = cold_outputs.clone();
        let cold_cleanup = cold_outputs.clone();
        let cold_wat = wat.clone();
        h.bench_batched(
            &format!("compile_cold/fn{n}"),
            move || {
                if let Some((wf, engine, dir)) = cold_cleanup.borrow_mut().take() {
                    drop(wf);
                    drop(engine);
                    drop(dir);
                }
                let dir = tempfile::TempDir::new().expect("tempdir");
                let engine =
                    Arc::new(WasmEngine::with_compile_cache(dir.path().to_path_buf()).unwrap());
                (engine, dir)
            },
            move |(engine, dir)| {
                let wf = WasmFunction::from_wat(engine.clone(), &cold_wat, WasmLimits::default())
                    .unwrap();
                std::hint::black_box(&wf);
                *cold_outputs.borrow_mut() = Some((wf, engine, dir));
            },
        );

        let warm_outputs = warm_outputs.clone();
        let warm_cleanup = warm_outputs.clone();
        let seed_wat = wat.clone();
        let warm_wat = wat.clone();
        h.bench_batched(
            &format!("compile_warm/fn{n}"),
            move || {
                if let Some((wf, engine, dir)) = warm_cleanup.borrow_mut().take() {
                    drop(wf);
                    drop(engine);
                    drop(dir);
                }
                let dir = tempfile::TempDir::new().expect("tempdir");
                let seeder =
                    Arc::new(WasmEngine::with_compile_cache(dir.path().to_path_buf()).unwrap());
                let seeded =
                    WasmFunction::from_wat(seeder.clone(), &seed_wat, WasmLimits::default())
                        .unwrap();
                drop(seeded);
                drop(seeder);
                let cache_root = dir.path().join("cache").join("wasm");
                assert!(
                    has_cache_entry_files(&cache_root),
                    "compile_warm/fn{n}: no .e cache entry under {}",
                    cache_root.display()
                );
                let engine =
                    Arc::new(WasmEngine::with_compile_cache(dir.path().to_path_buf()).unwrap());
                (engine, dir)
            },
            move |(engine, dir)| {
                let wf = WasmFunction::from_wat(engine.clone(), &warm_wat, WasmLimits::default())
                    .unwrap();
                std::hint::black_box(&wf);
                *warm_outputs.borrow_mut() = Some((wf, engine, dir));
            },
        );
    }

    // ── Group 5: concurrent calls (pooling vs on-demand) ────────────────
    // Shares one pre-compiled function across N tasks; set SHAMIR_WASM_NO_POOL=1 to compare allocators.
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
    // The final batched output survives until the harness run finishes; keep
    // its destructor and tempdir cleanup outside every timed routine.
    drop(cold_outputs);
    drop(warm_outputs);
}
