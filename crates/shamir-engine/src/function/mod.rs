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
mod context;
mod contract;
mod db_gateway;
mod env_policy;
mod error;
mod meta;
mod net_gateway;
mod params;
mod registry;
mod wasm;

pub use builtin::Argon2idFunction;
pub use compile::compile_rust_source;
pub use context::{BatchContext, FnBatch, FnCtx, GlobalVars};
pub use contract::ShamirFunction;
pub use db_gateway::DbGateway;
pub use env_policy::EnvPolicy;
pub use error::{FnResult, FunctionError};
pub use meta::{CreateFunctionOptions, FunctionMeta, Security, Visibility};
pub use net_gateway::{check_url_allowed, HttpRequest, HttpResponse, NetGateway};
pub use params::Params;
pub use registry::FunctionRegistry;
pub use wasm::{WasmEngine, WasmFunction, WasmLimits};

#[cfg(test)]
mod tests;
