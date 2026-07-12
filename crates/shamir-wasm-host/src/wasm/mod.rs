//! Wasmtime execution backend for user-defined functions.
//!
//! Re-exports the public surface:
//! * [`WasmEngine`] / [`WasmLimits`] — engine configuration (`wasm_engine`).
//! * [`WasmFunction`] — the [`ShamirFunction`] implementation (`wasm_function`).

mod host_batch;
mod host_call;
mod host_db;
mod host_globals;
mod host_http;
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
