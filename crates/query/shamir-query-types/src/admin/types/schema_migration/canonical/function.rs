use super::body::{Encoder, SchemaMigrationCanonicalError};
use crate::batch::BatchOp;

pub(super) fn existing_encoding(
    operation: BatchOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    crate::hmac::canonical_batch_op("", &operation)
        .map_err(|_| SchemaMigrationCanonicalError::ExistingOperationEncoding)
}

pub(super) fn encode_create_function(
    operation: &crate::admin::types::function_ops::CreateFunctionOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let mut bytes = existing_encoding(BatchOp::CreateFunction(operation.clone()))?;
    let mut extension = Encoder::new();
    extension.optional(operation.source.as_ref(), |encoder, source| {
        encoder.str(source)
    })?;
    extension.optional(operation.wasm.as_ref(), |encoder, wasm| encoder.str(wasm))?;
    bytes.extend_from_slice(&extension.bytes);
    Ok(bytes)
}

pub(super) fn encode_create_validator(
    operation: &crate::admin::types::validator_ops::CreateValidatorOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let mut bytes = existing_encoding(BatchOp::CreateValidator(operation.clone()))?;
    let mut extension = Encoder::new();
    extension.optional(operation.source.as_ref(), |encoder, source| {
        encoder.str(source)
    })?;
    extension.optional(operation.wasm.as_ref(), |encoder, wasm| encoder.str(wasm))?;
    bytes.extend_from_slice(&extension.bytes);
    Ok(bytes)
}
