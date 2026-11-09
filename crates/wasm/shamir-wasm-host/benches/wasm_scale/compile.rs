//! Compile cost by module size and code generator, plus what a compiled module costs to hold.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wasm_host::{TierMode, WasmEngine, WasmFunction, WasmLimits};

use crate::alloc;

/// A named module of a known size.
pub type Sized = (&'static str, Arc<Vec<u8>>);

pub fn register(h: &mut Harness, engine: &Arc<WasmEngine>, modules: &[Sized]) {
    for (id, bytes) in modules {
        for (tier, fast) in [("fast", true), ("optimized", false)] {
            let engine = engine.clone();
            let bytes = bytes.clone();
            h.bench(&format!("compile/{id}_{tier}"), move || {
                let module = if fast {
                    engine.compile_fast(&bytes)
                } else {
                    engine.compile_optimized(&bytes)
                }
                .expect("compile");
                black_box(&module);
            });
        }
    }
}

/// Prints the size of the compiled code per tier and the heap a loaded function holds.
///
/// `text` is the native code section; `image` is the serialized module (code plus metadata).
/// Linear memories live in the pooling allocator's mapped slots, whose ceiling is printed
/// once; the heap numbers are only what the host keeps on the Rust heap per function.
pub fn print_memory(engine: &Arc<WasmEngine>, modules: &[Sized]) {
    println!("== memory ==");
    match engine.pooling_memory_ceiling() {
        Some(ceiling) => println!("mem/pooling_memory_ceiling_bytes {ceiling}"),
        None => println!("mem/pooling_memory_ceiling_bytes none"),
    }
    for (id, bytes) in modules {
        let fast = engine.compile_fast(bytes).expect("fast compile");
        let optimized = engine.compile_optimized(bytes).expect("optimized compile");
        println!(
            "mem/{id}: wasm={} fast_text={} optimized_text={} fast_image={} optimized_image={}",
            bytes.len(),
            fast.text().len(),
            optimized.text().len(),
            fast.serialize().expect("serialize").len(),
            optimized.serialize().expect("serialize").len(),
        );
        for (tier, mode) in [("fast", TierMode::Never), ("optimized", TierMode::Now)] {
            let (function, live, peak) = alloc::measure(|| {
                WasmFunction::from_binary_with_mode(
                    engine.clone(),
                    bytes,
                    WasmLimits::default(),
                    mode,
                )
                .expect("function loads")
            });
            println!("mem/{id}_{tier}_function: heap_live={live} heap_peak={peak}");
            black_box(function);
        }
    }
}
