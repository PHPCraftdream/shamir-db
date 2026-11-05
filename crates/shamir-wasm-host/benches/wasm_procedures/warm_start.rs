//! Warm start: calling an already compiled procedure (instantiate + call).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_types::types::value::QueryValue;
use shamir_wasm_host::{
    FnBatch, FnCtx, Params, ShamirFunction, WasmEngine, WasmFunction, WasmLimits,
};

use crate::kernels;

/// Parallel callers in `warm/concurrent_8`.
const CONCURRENT: usize = 8;

pub fn register(h: &mut Harness) {
    let engine = Arc::new(WasmEngine::new().expect("engine"));
    let function = Arc::new(
        WasmFunction::from_wat(engine, &kernels::identity(), WasmLimits::default())
            .expect("compile"),
    );

    for (id, blob_bytes) in [
        ("call_identity", 0usize),
        ("call_params_4k", 4 * 1024),
        ("call_params_32k", 32 * 1024),
    ] {
        let function = function.clone();
        let mut params = Params::new();
        params.set("x", QueryValue::Int(42));
        if blob_bytes > 0 {
            params.set("blob", QueryValue::Str("x".repeat(blob_bytes)));
        }
        let ctx = FnCtx::new();
        let batch = FnBatch::new();
        h.bench_async(&format!("warm/{id}"), move || {
            let function = function.clone();
            let params = params.clone();
            let ctx = ctx.clone();
            let batch = batch.clone();
            async move {
                black_box(function.call(&ctx, &batch, &params).await.expect("call"));
            }
        });
    }

    let params = Params::new();
    h.bench_async(&format!("warm/concurrent_{CONCURRENT}"), move || {
        let function = function.clone();
        let params = params.clone();
        async move {
            let handles: Vec<_> = (0..CONCURRENT)
                .map(|_| {
                    let function = function.clone();
                    let params = params.clone();
                    tokio::spawn(async move {
                        function
                            .call(&FnCtx::new(), &FnBatch::new(), &params)
                            .await
                            .expect("call")
                    })
                })
                .collect();
            for handle in handles {
                black_box(handle.await.expect("join"));
            }
        }
    });
}
