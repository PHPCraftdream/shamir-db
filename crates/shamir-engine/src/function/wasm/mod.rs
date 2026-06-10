//! Wasmtime execution backend for user-defined functions.
//!
//! Re-exports the public surface:
//! * [`WasmEngine`] / [`WasmLimits`] — engine configuration (`wasm_engine`).
//! * [`WasmFunction`] — the [`ShamirFunction`] implementation (`wasm_function`).

mod wasm_engine;
mod wasm_function;

pub use wasm_engine::{WasmEngine, WasmLimits};
pub use wasm_function::WasmFunction;
