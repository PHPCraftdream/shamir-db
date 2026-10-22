//! Wasmtime execution backend for user-defined functions.
//!
//! Re-exports the public surface:
//! * [`WasmEngine`] / [`WasmLimits`] — engine configuration (`wasm_engine`).
//! * [`WasmFunction`] — the [`ShamirFunction`] implementation (`wasm_function`).

mod bindings;
pub(crate) mod compile_cache;
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

/// Test-only re-export: lets `tests::runtime::compile_tests` exercise the
/// `JoinError` mapping path of [`WasmFunction::from_binary_blocking`] by
/// spawning a deliberately panicking closure through the shared
/// `spawn_blocking` helper.
#[cfg(test)]
pub(crate) use wasm_function::run_compile_blocking;
