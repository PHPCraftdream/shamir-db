//! WASM adapter for [`RecordValidator`].
//!
//! [`WasmRecordValidator`] wraps an existing [`ShamirFunction`] (a compiled
//! WASM module) and implements the narrow [`RecordValidator`] interface.
//!
//! The adapter materialises `&dyn RecordFields` into a `QueryValue` map
//! internally — de-interning and msgpack packing happen here, at the ABI
//! boundary, and are paid **only** by WASM validators.  Native/declarative
//! validators never pay this cost.
//!
//! Guest ABI is unchanged: the WASM guest still receives
//! `Params { "record": QueryValue, "old_record": QueryValue }` packed into
//! msgpack linear memory (see `wasm_function.rs:302`).

use std::sync::Arc;

use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use shamir_wasm_host::{FnBatch, FnCtx, Params, ShamirFunction};

use super::{
    decode_validation_result,
    record_fields::RecordFields,
    record_validator::{RecordValidator, ValidatorCtx},
    Validation,
};

/// WASM-backed [`RecordValidator`].
///
/// Wraps a compiled [`ShamirFunction`] (WASM module) and bridges from the
/// `&dyn RecordFields` interface to the `ShamirFunction::call` ABI.
///
/// The de-intern + msgpack cost is localised here — only WASM validators pay
/// it.
pub struct WasmRecordValidator {
    function: Arc<dyn ShamirFunction>,
}

impl WasmRecordValidator {
    /// Wrap a compiled WASM function as a validator.
    pub fn new(function: Arc<dyn ShamirFunction>) -> Self {
        Self { function }
    }
}

#[async_trait]
impl RecordValidator for WasmRecordValidator {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        old: Option<&dyn RecordFields>,
        ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        // 1. Build FnCtx with the actor from ValidatorCtx.
        let fn_ctx = FnCtx::new().with_actor(ctx.actor.clone());
        let batch = FnBatch::new();

        // 2. Materialise the records into QueryValue (de-intern happens here;
        //    only WASM pays this cost).
        let qv_new = new.map(|f| f.to_query_value()).unwrap_or(QueryValue::Null);
        let qv_old = old.map(|f| f.to_query_value()).unwrap_or(QueryValue::Null);

        // 3. Build Params — mirrors table_manager_validators.rs:245-255.
        let mut params = Params::new();
        params.set("record", qv_new);
        params.set("old_record", qv_old);

        // 4. Invoke the WASM guest.
        let result = match self.function.call(&fn_ctx, &batch, &params).await {
            Ok(v) => v,
            Err(e) => {
                // Invocation failure — surface as a stop-rejection with a
                // sentinel code so `run_validators_loop` can detect it.
                return wasm_invocation_error(e.to_string());
            }
        };

        // 5. Decode the guest's return value.
        match decode_validation_result(&result) {
            Ok(outcome) => Validation {
                errors: outcome.errors,
                stop: outcome.stop,
            },
            Err(e) => wasm_invocation_error(e.to_string()),
        }
    }
}

fn wasm_invocation_error(reason: String) -> Validation {
    let mut v = Validation::default();
    v.errors
        .push(shamir_query_types::validator::ValidationError {
            field: None,
            code: format!("__wasm_err:{reason}"),
        });
    v.stop = true;
    v
}
