//! Inner computation: each kernel as a compiled guest and as plain Rust.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wasm_host::{
    FnBatch, FnCtx, Params, ShamirFunction, TierMode, WasmEngine, WasmFunction, WasmLimits,
};

use crate::kernels;

pub fn register(h: &mut Harness) {
    let engine = Arc::new(WasmEngine::new().expect("engine"));

    for kernel in kernels::KERNELS {
        // Pinned to the optimized tier (`TierMode::Now`): the default mode
        // `Auto` would start each kernel on the fast tier and upgrade it
        // mid-run, mixing tiers inside one cell.
        let function = Arc::new(
            WasmFunction::from_wat_with_mode(
                engine.clone(),
                &(kernel.wat)(),
                WasmLimits::default(),
                TierMode::Now,
            )
            .expect("compile"),
        );
        let ctx = FnCtx::new();
        let batch = FnBatch::new();
        let params = Params::new();
        h.bench_async(&format!("compute/wasm/{}", kernel.name), move || {
            let function = function.clone();
            let ctx = ctx.clone();
            let batch = batch.clone();
            let params = params.clone();
            async move {
                black_box(function.call(&ctx, &batch, &params).await.expect("call"));
            }
        });

        let native = kernel.native;
        h.bench(&format!("compute/native/{}", kernel.name), move || {
            black_box(native());
        });
    }

    // Fast-tier twins (`TierMode::Never`): the per-call cost the optimized
    // tier is measured against in the tier trade-off report.
    for kernel in kernels::KERNELS {
        let function = Arc::new(
            WasmFunction::from_wat_with_mode(
                engine.clone(),
                &(kernel.wat)(),
                WasmLimits::default(),
                TierMode::Never,
            )
            .expect("compile"),
        );
        let ctx = FnCtx::new();
        let batch = FnBatch::new();
        let params = Params::new();
        h.bench_async(&format!("compute/wasm_fast/{}", kernel.name), move || {
            let function = function.clone();
            let ctx = ctx.clone();
            let batch = batch.clone();
            let params = params.clone();
            async move {
                black_box(function.call(&ctx, &batch, &params).await.expect("call"));
            }
        });
    }
}
