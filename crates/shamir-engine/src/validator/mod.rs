//! Validator types for the engine layer.
//!
//! Re-exports wire-facing types from `shamir_query_types::validator` and
//! adds engine-internal structs: `ValidatorBinding`, `PersistedValidators`,
//! `ValidationOutcome`, `ValidatorDecodeError`, the ABI decoder, the
//! lock-free `ValidatorRegistry`, and the S3 write-path failure type
//! `ValidatorFailure`.

pub mod persistence;
mod registry;

mod decode;
mod persisted_validators;
mod query_value_conv;
mod validation_outcome;
mod validator_binding;

pub use decode::{decode_validation_result, ValidatorDecodeError};
pub use persisted_validators::PersistedValidators;
pub use query_value_conv::{inner_to_query_value, inner_to_query_value_with};
pub use validation_outcome::{ValidationOutcome, ValidatorFailure};
pub use validator_binding::ValidatorBinding;

pub use registry::{ValidatorRegistry, ValidatorRegistryError};

// -- re-exports from the wire crate -----------------------------------------
pub use shamir_query_types::validator::{ValidationError, WriteOp};

#[cfg(test)]
mod tests;
