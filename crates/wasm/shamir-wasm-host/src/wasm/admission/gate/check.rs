//! The dry-run check: admit, compile and pre-instantiate, persisting nothing.

use super::compile_gate::run_gated_compile;
use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::admission::{admit_module, AdmissionPolicy, ModuleReport};
use crate::wasm::engine::{TierMode, WasmEngine};
use crate::wasm::wasm_function::build_linker;
use std::sync::Arc;
use wasmtime::{Engine, Module};

/// Dry run of a module creation: admits `bytes` under `policy`, compiles them
/// the way the creation path decides for `mode` (`Auto`: fast tier, falling
/// back to the optimized tier; `Never`: the fast tier only, a rejection is an
/// error; `Now`: the optimized tier), builds the `InstancePre` and discards
/// everything.
///
/// Every optimized-tier compile runs on a cache-less engine, so the
/// incremental disk compile cache is never written; the fast tier never uses
/// it. Runs under the compile gate. Nothing is registered or persisted.
pub async fn check_module(
    engine: Arc<WasmEngine>,
    bytes: &[u8],
    policy: AdmissionPolicy,
    mode: TierMode,
) -> FnResult<ModuleReport> {
    let wasm = bytes.to_vec();
    let gate = Arc::clone(&engine);
    // `AdmissionPolicy` is `Copy`, so `policy` is captured by the same
    // by-value move as before and shared with the ungated body by reference.
    run_gated_compile(&gate, &policy, move |_permit| {
        check_module_ungated(&engine, &wasm, &policy, mode)
    })
    .await
}

/// The gate-free body of [`check_module`]: admits `wasm` under `policy` and
/// compiles/pre-instantiates for `mode`, persisting nothing. Called inside
/// a `run_gated_compile` closure — either `check_module`'s own single
/// permit or the shamir source service's — never with a gate of its own.
pub(in crate::wasm) fn check_module_ungated(
    engine: &Arc<WasmEngine>,
    wasm: &[u8],
    policy: &AdmissionPolicy,
    mode: TierMode,
) -> FnResult<ModuleReport> {
    let report = admit_module(wasm, policy)?;
    compile_and_discard(engine, wasm, mode)?;
    Ok(report)
}

fn compile_and_discard(engine: &WasmEngine, wasm: &[u8], mode: TierMode) -> FnResult<()> {
    match mode {
        TierMode::Now => check_optimized(engine, wasm),
        TierMode::Never => match engine.fast_engine() {
            Some(fast_engine) => {
                let module = engine.compile_fast(wasm).map_err(compile_error)?;
                pre_instantiate(fast_engine, &module)
            }
            None => check_optimized(engine, wasm),
        },
        TierMode::Auto => match engine.fast_engine() {
            Some(fast_engine) => match engine.compile_fast(wasm) {
                Ok(module) => pre_instantiate(fast_engine, &module),
                Err(fast_error) => {
                    log::info!(
                        "wasm check: fast tier compile failed ({fast_error}); \
                         checking the optimized tier"
                    );
                    check_optimized(engine, wasm)
                }
            },
            None => check_optimized(engine, wasm),
        },
    }
}

fn check_optimized(engine: &WasmEngine, wasm: &[u8]) -> FnResult<()> {
    let uncached = engine.uncached_optimized_engine()?;
    let module = Module::from_binary(uncached, wasm).map_err(compile_error)?;
    pre_instantiate(uncached, &module)
}

fn pre_instantiate(engine: &Engine, module: &Module) -> FnResult<()> {
    let linker = build_linker(engine)?;
    let _pre = linker
        .instantiate_pre(module)
        .map_err(|e| FunctionError::Compute(format!("instantiate_pre failed: {e}")))?;
    Ok(())
}

fn compile_error(error: wasmtime::Error) -> FunctionError {
    FunctionError::Compute(format!("WASM compile error: {error}"))
}
