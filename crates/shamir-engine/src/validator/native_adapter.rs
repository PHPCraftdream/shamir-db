//! Adapter that turns a human-friendly validation closure into a
//! [`ShamirFunction`] the engine can invoke through the standard validator
//! call path.
//!
//! The closure receives `(&QueryValue, Option<&QueryValue>, &FnCtx)` —
//! `record`, `old_record`, and the function context — and returns a
//! [`Validation`]. The adapter encodes that to [`QueryValue`] via
//! [`validation_to_query_value`], producing the Map form that
//! [`decode_validation_result`] consumes. No call-site change is needed in
//! `table_manager_validators.rs`.

use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use shamir_wasm_host::{FnBatch, FnCtx, FnResult, Params, ShamirFunction};

use crate::validator::{validation_to_query_value, Validation};

/// Type alias for the user-supplied validation closure.
pub type NativeValidatorFn =
    dyn Fn(&QueryValue, Option<&QueryValue>, &FnCtx) -> Validation + Send + Sync;

/// Wraps a native validation closure so it satisfies [`ShamirFunction`].
pub struct NativeValidatorAdapter {
    /// The boxed closure.
    pub validator: Box<NativeValidatorFn>,
}

impl NativeValidatorAdapter {
    /// Create from any closure matching the native-validator signature.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&QueryValue, Option<&QueryValue>, &FnCtx) -> Validation + Send + Sync + 'static,
    {
        Self {
            validator: Box::new(f),
        }
    }
}

#[async_trait]
impl ShamirFunction for NativeValidatorAdapter {
    async fn call(&self, ctx: &FnCtx, _batch: &FnBatch, params: &Params) -> FnResult<QueryValue> {
        // Extract record / old_record from params (pinned by
        // table_manager_validators.rs:228-237).
        let record = params.get("record")?;
        let old_record = params.get("old_record")?;

        // The engine passes Null when there is no old/new value; translate
        // Null → None so the closure gets a clean Option.
        let old_ref = match old_record {
            QueryValue::Null => None,
            other => Some(other),
        };

        let result = (self.validator)(record, old_ref, ctx);

        // Accept path: skip the Map-encode round trip entirely.
        // `decode_validation_result` already fast-paths `QueryValue::Null` as
        // "valid, empty errors, stop=false" (decode.rs) — returning `Null`
        // directly here avoids allocating `{ "errors": [], "stop": false }`
        // on every accepting invocation just to decode it straight back into
        // the same empty outcome. A `stop == true` accept (no errors, but the
        // validator still requests early-stop) must NOT take this shortcut:
        // `Null` decodes to `stop = false`, which would silently drop that
        // request.
        if result.is_ok() && !result.stop {
            return Ok(QueryValue::Null);
        }
        Ok(validation_to_query_value(&result))
    }
}
