//! Benchmarks of the WASM procedure path (manual runs only, never part of the regular CI).
//!
//! Cells, grouped by the question they answer:
//!
//! * `cold/*` — cold start: engine creation, compile of a module, compile + first call.
//! * `warm/*` — warm start: a call of an already compiled procedure (instantiate + call),
//!   with growing parameter payloads and under 8-way concurrency.
//! * `compute/wasm/*` vs `compute/native/*` — the same kernels in a guest and as plain Rust;
//!   the ratio is the "how far from native" number.
//! * `host/*` — guest/host boundary: 100 sync (`batch_put`) or async (`batch_get`) host
//!   calls per invocation. Per-call cost = (cell − `warm/call_identity`) / 100.
//! * `metering/*` — the same kernels on a bare engine with fuel and/or epoch interruption
//!   on or off: what the sandbox's metering costs.
//!
//! Real database calls from a guest live in `shamir-db` (`wasm_db_calls`).
//!
//! The code generator is picked with `SHAMIR_WASM_STRATEGY=cranelift|winch|pulley`
//! (needs `--features bench-strategies`); an unsupported alternative prints `SKIP` and exits
//! successfully, a failing default strategy panics. The whole run is sized to stay within about 15 seconds: iteration counts are
//! small and pinned in `bench-iters.txt` (`-- --calibrate 0.1` regenerates them).

mod cold_start;
mod compute;
mod host_boundary;
mod kernels;
mod metering;
mod warm_start;

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_types::types::value::QueryValue;
use shamir_wasm_host::{
    FnBatch, FnCtx, Params, ShamirFunction, WasmEngine, WasmFunction, WasmLimits,
};

/// Compiles and runs every guest once. A strategy that cannot do that is reported as
/// unsupported; a kernel that disagrees with its native twin panics (the ratio would lie).
fn preflight() -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let engine = Arc::new(WasmEngine::new().map_err(|error| format!("engine: {error}"))?);
    let run = |wat: String, batch: &FnBatch| -> Result<QueryValue, String> {
        let function = WasmFunction::from_wat(engine.clone(), &wat, WasmLimits::default())
            .map_err(|error| format!("compile: {error}"))?;
        rt.block_on(function.call(&FnCtx::new(), batch, &Params::new()))
            .map_err(|error| format!("call: {error}"))
    };

    let plain = FnBatch::new();
    for kernel in kernels::KERNELS {
        let got =
            run((kernel.wat)(), &plain).map_err(|error| format!("{}: {error}", kernel.name))?;
        let want = QueryValue::Int(i64::from((kernel.native)() & 127));
        assert_eq!(
            got, want,
            "kernel `{}` disagrees with its native twin",
            kernel.name
        );
    }

    let with_key = FnBatch::new();
    with_key.context().put("k", QueryValue::Int(7));
    assert_eq!(
        run(kernels::sync_put_wat(), &with_key).map_err(|error| format!("sync_put: {error}"))?,
        QueryValue::Int(7)
    );
    assert_eq!(
        run(kernels::async_get_wat(), &with_key).map_err(|error| format!("async_get: {error}"))?,
        QueryValue::Int(7)
    );
    Ok(())
}

fn main() {
    let strategy = std::env::var("SHAMIR_WASM_STRATEGY").unwrap_or_else(|_| "cranelift".to_owned());
    println!("== wasm_procedures: strategy={strategy} ==");
    if let Err(reason) = preflight() {
        // Only an alternative strategy may be unsupported; for the default one a failure
        // is a bug in the guests or the host, and must not pass as a skipped run.
        assert!(
            strategy != "cranelift",
            "preflight failed on the default strategy: {reason}"
        );
        println!("SKIP strategy={strategy}: {reason}");
        return;
    }

    let mut h = Harness::new("wasm_procedures", env!("CARGO_MANIFEST_DIR"));
    cold_start::register(&mut h);
    warm_start::register(&mut h);
    compute::register(&mut h);
    host_boundary::register(&mut h);
    metering::register(&mut h);
    h.run();
}
