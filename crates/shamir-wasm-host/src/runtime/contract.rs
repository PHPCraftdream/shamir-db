//! The function authoring contract.

use super::context::{FnBatch, FnCtx};
use super::error::FnResult;
use super::params::Params;
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;

/// A user-defined function: an async mapping `(ctx, batch, params) -> value`
/// evaluated inside the transaction.
///
/// One contract serves every call site (batch node, `where`, `set`, key
/// generation); the site binds `params` and reinterprets the returned
/// [`QueryValue`]. The name a function is invoked by lives in the
/// [`FunctionRegistry`](super::FunctionRegistry), not here — the artifact is
/// content, the name is a catalogue pointer, so rename never recompiles.
///
/// Implementations that do CPU- or memory-bound work MUST offload it (e.g.
/// `tokio::task::spawn_blocking`) so the async runtime's worker threads stay
/// free — see [`Argon2idFunction`](super::Argon2idFunction).
#[async_trait]
pub trait ShamirFunction: Send + Sync {
    /// Evaluate the function.
    async fn call(&self, ctx: &FnCtx, batch: &FnBatch, params: &Params) -> FnResult<QueryValue>;
}
