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
mod context;
mod contract;
mod error;
mod params;
mod registry;

pub use builtin::Argon2idFunction;
pub use context::{FnBatch, FnCtx};
pub use contract::ShamirFunction;
pub use error::{FnResult, FunctionError};
pub use params::Params;
pub use registry::FunctionRegistry;

#[cfg(test)]
mod tests;
