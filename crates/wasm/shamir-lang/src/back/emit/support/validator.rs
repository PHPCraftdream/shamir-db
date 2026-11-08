//! Validator `shamir_call` wrapper emission. The validator wrapper shares the
//! scalar envelope machinery (numeric-key/positional, named, and `args`
//! forms; optional-parameter null fill; deep typed projection; result boxing;
//! canonical encode) and only narrows the contract: the parameters `record`
//! and `old_record` are both optional records — an absent key or a null
//! decodes to the null cell, a map is deep-projected against the declared
//! record type, adapter fields not declared in the parameter type are
//! ignored, and a missing required declared field or a wrongly typed one is
//! a kind-2 failure. The result is the `Validation` record whose encoding
//! through the shared encoder (records dispatch enabled) is exactly the
//! canonical `{ "errors": [ { "code": "..", "field": [..] } ], "stop": bool }`
//! bytes — key order `errors` then `stop`, per-error key order `code` then
//! `field`, `field` omitted for record-level errors, one segment for
//! `reject`, and the path for `reject_path`.

use wasm_encoder::Function;

use crate::back::emit::module::EmitError;
use crate::back::ir::ParamProjection;

/// Emits the validator `shamir_call` wrapper: first the validator invariants
/// are checked (exactly two optional-cell parameters — `record` and
/// `old_record` — and a plain-cell `Validation` result; a violation is an
/// internal invariant), then emission is delegated to the shared scalar
/// wrapper, which already implements the envelope scan, optional-parameter
/// null fill, deep typed projection, result boxing and canonical encode.
pub(crate) fn emit_validator_wrapper(
    inputs: crate::back::emit::abi::WrapperInputs,
) -> Result<Function, EmitError> {
    let expected: [ParamProjection; 2] =
        [ParamProjection::OptionalCell, ParamProjection::OptionalCell];
    if inputs.param_kinds != expected.as_slice() {
        return Err(EmitError::Internal(
            "validator wrapper needs exactly two optional-cell parameters",
        ));
    }
    if inputs.result_projection != Some(ParamProjection::Cell) {
        return Err(EmitError::Internal(
            "validator wrapper result must project as a cell",
        ));
    }
    crate::back::emit::abi::emit_scalar_wrapper(inputs)
}
