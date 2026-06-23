//! Closure → [`ShamirFunction`] adapter.
//!
//! A blanket `impl<F: Fn(...)> ShamirFunction for F` fails Rust's coherence
//! rules (Phase 0 Seam 5 verdict), so this concrete wrapper is the sanctioned
//! way to register an ad-hoc async closure as a first-class procedural
//! function via [`FunctionRegistry::register`].
//!
//! ```ignore
//! registry.register("my_fn", Arc::new(FnAdapter(
//!     |_ctx, _batch, _params| async move { Ok(QueryValue::Null) },
//! )))?;
//! ```

use crate::context::{FnBatch, FnCtx};
use crate::contract::ShamirFunction;
use crate::error::FnResult;
use crate::params::Params;
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::future::Future;

/// Wraps a closure `F: Fn(&FnCtx, &FnBatch, &Params) -> Fut` so it satisfies
/// `ShamirFunction`.
pub struct FnAdapter<F>(pub F);

#[async_trait]
impl<F, Fut> ShamirFunction for FnAdapter<F>
where
    F: Fn(&FnCtx, &FnBatch, &Params) -> Fut + Send + Sync,
    Fut: Future<Output = FnResult<QueryValue>> + Send,
{
    async fn call(&self, ctx: &FnCtx, batch: &FnBatch, params: &Params) -> FnResult<QueryValue> {
        self.0(ctx, batch, params).await
    }
}
