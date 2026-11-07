use async_trait::async_trait;
use shamir_engine::function::{FnBatch, FnCtx, FnResult, FunctionError, Params, ShamirFunction};
use shamir_types::types::value::QueryValue;

use super::FunctionState;

/// Placeholder registered for a function whose stored module cannot run;
/// every call fails closed with the recorded reason.
pub struct UnavailableFunction {
    name: String,
    state: FunctionState,
}

impl UnavailableFunction {
    /// Build a placeholder for `name` in the given (non-loaded) state.
    pub fn new(name: &str, state: FunctionState) -> Self {
        Self {
            name: name.to_string(),
            state,
        }
    }
}

#[async_trait]
impl ShamirFunction for UnavailableFunction {
    async fn call(&self, _ctx: &FnCtx, _batch: &FnBatch, _params: &Params) -> FnResult<QueryValue> {
        let msg = self
            .state
            .unavailable_message(&self.name)
            .unwrap_or_else(|| format!("function '{}' is unavailable", self.name));
        Err(FunctionError::Compute(msg))
    }
}
