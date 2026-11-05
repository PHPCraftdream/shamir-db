//! Cold start: creating an engine, compiling a module, compiling and calling it once.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wasm_host::{
    FnBatch, FnCtx, Params, ShamirFunction, WasmEngine, WasmFunction, WasmLimits,
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
        h.bench(&format!("cold/compile_{id}"), move || {
            let function = WasmFunction::from_wat(engine.clone(), &wat, WasmLimits::default())
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
                let function =
                    WasmFunction::from_wat(engine, &wat, WasmLimits::default()).expect("compile");
                let out = function
                    .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
                    .await
                    .expect("call");
                black_box(out);
            }
        });
    }
}
