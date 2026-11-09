//! Call latency while a background compile runs.
//!
//! The two-tier engine compiles the optimized version of a function on the blocking pool
//! while callers keep using the fast one. This measures the tail of a call on the fast
//! tier with no background work, and with one and four threads compiling a large module on
//! the optimized tier without pause. The numbers are printed as `bg/<case>` lines: the
//! harness times a mean, and a tail needs the distribution.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use shamir_wasm_host::{
    FnBatch, FnCtx, Params, ShamirFunction, TierMode, WasmEngine, WasmFunction, WasmLimits,
};

use crate::modules;

const CALLS: usize = 100_000;

fn sample(runtime: &tokio::runtime::Runtime, function: &WasmFunction) -> Vec<u64> {
    let ctx = FnCtx::new();
    let batch = FnBatch::new();
    let params = Params::new();
    let mut nanos = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        let started = Instant::now();
        runtime
            .block_on(function.call(&ctx, &batch, &params))
            .expect("call");
        nanos.push(started.elapsed().as_nanos() as u64);
    }
    nanos.sort_unstable();
    nanos
}

fn print(case: &str, sorted: &[u64]) {
    let at = |fraction: f64| sorted[((sorted.len() - 1) as f64 * fraction) as usize];
    println!(
        "bg/{case}: calls={} p50={}ns p99={}ns p999={}ns max={}ns",
        sorted.len(),
        at(0.5),
        at(0.99),
        at(0.999),
        sorted[sorted.len() - 1],
    );
}

pub fn report(engine: &Arc<WasmEngine>, heavy: &Arc<Vec<u8>>) {
    println!("== background compile ==");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let function = WasmFunction::from_binary_with_mode(
        engine.clone(),
        &modules::binary(0),
        WasmLimits::default(),
        TierMode::Never,
    )
    .expect("fast function");
    print("idle", &sample(&runtime, &function));
    for threads in [1usize, 4] {
        let stop = Arc::new(AtomicBool::new(false));
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let engine = engine.clone();
                let heavy = heavy.clone();
                let stop = stop.clone();
                thread::spawn(move || {
                    let mut compiles = 0usize;
                    while !stop.load(Ordering::Relaxed) {
                        engine
                            .compile_optimized(&heavy)
                            .expect("background compile");
                        compiles += 1;
                    }
                    compiles
                })
            })
            .collect();
        let sorted = sample(&runtime, &function);
        stop.store(true, Ordering::Relaxed);
        let compiles: usize = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker"))
            .sum();
        print(&format!("compile_x{threads}"), &sorted);
        println!("bg/compile_x{threads}: finished_compiles={compiles}");
    }
}
