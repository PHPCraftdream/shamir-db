//! Cold start: creating an engine, compiling a module, compiling and calling it once.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wasm_host::{
    FnBatch, FnCtx, Params, ShamirFunction, TierMode, WasmEngine, WasmFunction, WasmLimits,
};

use crate::kernels;

pub fn register(h: &mut Harness) {
    h.bench("cold/engine_new", || {
        black_box(WasmEngine::new().expect("engine"));
    });

    let engine = Arc::new(WasmEngine::new().expect("engine"));

    for (id, wat) in [
        ("identity", kernels::identity()),
        ("fn64", kernels::many_functions(64)),
        ("int_sum", kernels::int_sum_wat()),
    ] {
        let engine = engine.clone();
        // Pinned to the optimized tier so the published cold-compile
        // numbers stay comparable (the default mode is now `Auto`).
        h.bench(&format!("cold/compile_{id}"), move || {
            let function = WasmFunction::from_wat_with_mode(
                engine.clone(),
                &wat,
                WasmLimits::default(),
                TierMode::Now,
            )
            .expect("compile");
            black_box(&function);
        });
    }

    for (id, wat) in [
        ("identity", kernels::identity()),
        ("int_sum", kernels::int_sum_wat()),
    ] {
        let engine = engine.clone();
        let wat = Arc::new(wat);
        h.bench_async(&format!("cold/compile_and_first_call_{id}"), move || {
            let engine = engine.clone();
            let wat = wat.clone();
            async move {
                let function = WasmFunction::from_wat_with_mode(
                    engine,
                    &wat,
                    WasmLimits::default(),
                    TierMode::Now,
                )
                .expect("compile");
                let out = function
                    .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
                    .await
                    .expect("call");
                black_box(out);
            }
        });
    }

    // Two-tier compile cells: the same module created through the fast
    // (Winch) and the optimized (Cranelift) path — the compile side of the
    // tier trade-off report.
    // One kernel-sized module (`int_sum`) stands for all compute kernels: their
    // guests are the same size, and each extra pair costs bench budget.
    for (id, wat) in [
        ("identity", kernels::identity()),
        ("fn64", kernels::many_functions(64)),
        ("int_sum", kernels::int_sum_wat()),
    ] {
        for (suffix, mode) in [("fast", TierMode::Never), ("optimized", TierMode::Now)] {
            let engine = engine.clone();
            let wat = wat.clone();
            h.bench(&format!("cold/compile_{id}_{suffix}"), move || {
                let function = WasmFunction::from_wat_with_mode(
                    engine.clone(),
                    &wat,
                    WasmLimits::default(),
                    mode,
                )
                .expect("compile");
                black_box(&function);
            });
        }
    }

    // Compile scaling: a much larger generated module through both tiers —
    // where the fast tier's compile win is concentrated.
    let big = Arc::new(kernels::many_functions(512));
    for (suffix, mode) in [("fast", TierMode::Never), ("optimized", TierMode::Now)] {
        let engine = engine.clone();
        let big = Arc::clone(&big);
        h.bench(&format!("cold/compile_fn512_{suffix}"), move || {
            let function =
                WasmFunction::from_wat_with_mode(engine.clone(), &big, WasmLimits::default(), mode)
                    .expect("compile");
            black_box(&function);
        });
    }

    // The new cold start: create in `Auto` (fast tier synchronously) and
    // call once. The background upgrade compiles off the measured path on
    // the blocking pool.
    let wat = Arc::new(kernels::identity());
    h.bench_async("tier/create_and_first_call_auto", move || {
        let engine = engine.clone();
        let wat = Arc::clone(&wat);
        async move {
            let function = WasmFunction::from_wat_with_mode(
                engine,
                &wat,
                WasmLimits::default(),
                TierMode::Auto,
            )
            .expect("compile");
            let out = function
                .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
                .await
                .expect("call");
            black_box(out);
        }
    });
}
