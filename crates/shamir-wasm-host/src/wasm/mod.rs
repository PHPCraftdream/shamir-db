//! Wasmtime execution backend for user-defined functions.
//!
//! Re-exports the public surface:
//! * [`WasmEngine`] / [`WasmLimits`] — engine configuration (`wasm_engine`).
//! * [`WasmFunction`] — the [`ShamirFunction`] implementation (`wasm_function`).

pub(crate) mod bindings;
pub(crate) mod compile_cache;
mod fuel;
mod wasm_engine;
mod wasm_function;
mod wasm_sanitizer;

pub use wasm_engine::{WasmEngine, WasmLimits};
pub use wasm_function::WasmFunction;
pub use wasm_sanitizer::{verify_wasm_module, SANCTIONED_HOST_IMPORTS};

/// Test-only re-export: lets
/// `tests::wasm_sanitizer_tests::sanctioned_list_matches_linker_registrations`
/// build the real registered [`wasmtime::Linker`] surface (see
/// `wasm_function::test_linker_and_store` for why) and enumerate it via
/// [`wasmtime::Linker::iter`].
#[cfg(test)]
pub(crate) use wasm_function::test_linker_and_store;

/// Test-only re-export: lets `tests::wasm::fuel::*` attach and read the
/// per-invocation fuel-lease observer (including its `lease_raii_probe`
/// helper for the reservation token's refund/defuse discipline).
#[cfg(test)]
pub(crate) use fuel::FuelObserver;

/// Re-export: lets `compile::compile_rust_source_blocking` offload the
/// synchronous pipeline via the shared `spawn_blocking` helper, and
/// `tests::runtime::compile_tests` exercise its `JoinError` mapping path.
pub(crate) use wasm_function::run_compile_blocking;
