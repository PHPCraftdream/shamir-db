//! Guest/host boundary: sync and async host imports called in a loop from a guest.
//!
//! Per-call cost = (cell − `warm/call_identity`) / [`kernels::HOST_CALLS`].

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_types::types::value::QueryValue;
use shamir_wasm_host::{
    FnBatch, FnCtx, Params, ShamirFunction, WasmEngine, WasmFunction, WasmLimits,
};

use crate::kernels;

pub fn register(h: &mut Harness) {
    let engine = Arc::new(WasmEngine::new().expect("engine"));
    let batch = FnBatch::new();
    batch.context().put("k", QueryValue::Int(7));

    for (id, wat) in [
        ("sync_put", kernels::sync_put_wat()),
        ("async_get", kernels::async_get_wat()),
    ] {
        let function = Arc::new(
            WasmFunction::from_wat(engine.clone(), &wat, WasmLimits::default()).expect("compile"),
        );
        let batch = batch.clone();
        let ctx = FnCtx::new();
        let params = Params::new();
        h.bench_async(&format!("host/{id}_x{}", kernels::HOST_CALLS), move || {
            let function = function.clone();
            let batch = batch.clone();
            let ctx = ctx.clone();
            let params = params.clone();
            async move {
                black_box(function.call(&ctx, &batch, &params).await.expect("call"));
            }
        });
    }
}
