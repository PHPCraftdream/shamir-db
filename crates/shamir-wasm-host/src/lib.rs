#![forbid(unsafe_code)]

//! User-defined function engine — execution model (slice 1).
//!
//! A function is an async mapping `(ctx, batch, params) -> value` evaluated
//! inside the transaction. The same contract serves every call site — batch
//! node, `where`, `set`, key generation — with the site binding `params` and
//! reinterpreting the returned value.
//!
//! This slice lands the EXECUTION MODEL: the [`ShamirFunction`] contract, a
//! lock-free [`FunctionRegistry`] (register / replace / rename / drop), and
//! the first built-in — [`Argon2idFunction`] — which runs its CPU- and
//! memory-bound KDF on `spawn_blocking` so the async runtime's worker
//! threads are never blocked.
//!
//! Subsequent slices add: Wasmtime `.cwasm` loading + the compile-on-DDL
//! pipeline, the durable function catalogue, fuel/memory limits, and the
//! real [`FnCtx`]/[`FnBatch`] bodies (DB access on the current `TxContext`
//! and the `@alias` batch namespace) — both are intentional placeholders in
//! this slice.

mod builtin;
mod compile;
mod gateway;
mod runtime;
mod wasm;

pub use builtin::{
    builtin_aggs, builtin_scalars, Argon2idFunction, CreateFunctionOptions, FunctionMeta, Security,
    Visibility,
};
pub use compile::{compile_rust_source, compile_rust_source_with_timeout, WASM_COMPILE_TIMEOUT};
pub use gateway::{
    check_url_allowed, check_url_allowed_resolved, DbGateway, EnvPolicy, HttpRequest, HttpResponse,
    NetGateway, ResolvedPin,
};
pub use runtime::{
    BatchContext, FnAdapter, FnBatch, FnCtx, FnResult, FunctionError, FunctionRegistry, GlobalVars,
    Params, ShamirFunction,
};
pub use wasm::{verify_wasm_module, WasmEngine, WasmFunction, WasmLimits, SANCTIONED_HOST_IMPORTS};

#[cfg(test)]
pub(crate) use wasm::test_linker_and_store;

#[cfg(test)]
pub(crate) use wasm::FuelObserver;

#[cfg(test)]
mod tests;
