//! Scaling benchmarks of the WASM procedure path (manual runs only, never part of the
//! regular CI): what the second stage of the compiler plan has to decide with numbers.
//!
//! Cells (`bench_scale_tool` lines):
//!
//! * `compile/<size>_{fast,optimized}` — compile of a generated module of about 1 KiB,
//!   10 KiB, 100 KiB and 1 MiB on the Winch and the Cranelift engine of the host.
//! * `load/{admit,hash}_<size>` — the admission pass and the module hash per size.
//! * `lang/compile_*` — `shamir-lang` source to a self-admitted module.
//!
//! Printed lines (not harness cells):
//!
//! * `mem/*` — compiled code size per tier and the heap a loaded function holds.
//! * `bg/*` — call latency percentiles with and without background compiles.
//!
//! The break-even number of calls of the optimized tier is
//! `(compile_optimized - compile_fast) / (call_fast - call_optimized)`, with the call
//! costs from `wasm_procedures` (`tier/*`, `warm/*`).
//!
//! The counting allocator below only counts while a measurement window is open (one
//! relaxed load per allocation otherwise).
//!
//! Counts are pinned in `bench-iters.txt` (`-- --calibrate 0.05` regenerates them).

mod alloc;
mod background;
mod compile;
mod lang;
mod load;
mod modules;

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wasm_host::{admit_module, AdmissionLimits, AdmissionPolicy, WasmEngine};

#[global_allocator]
static COUNTING: alloc::Counting = alloc::Counting;

fn main() {
    let engine = Arc::new(WasmEngine::new().expect("engine"));
    let modules: Vec<compile::Sized> = modules::SIZES
        .iter()
        .map(|(id, target)| (*id, Arc::new(modules::sized(*target))))
        .collect();
    println!("== modules ==");
    let policy = AdmissionPolicy::Admission(AdmissionLimits::default());
    for (id, bytes) in &modules {
        admit_module(bytes, &policy).expect("generated module is admitted");
        println!("module/{id}: wasm={}", bytes.len());
    }

    compile::print_memory(&engine, &modules);
    background::report(&engine, &modules[2].1);

    let mut h = Harness::new("wasm_scale", env!("CARGO_MANIFEST_DIR"));
    compile::register(&mut h, &engine, &modules);
    load::register(&mut h, &modules);
    lang::register(&mut h);
    h.run();
}
