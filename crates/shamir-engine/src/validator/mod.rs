//! Validator types for the engine layer.
//!
//! Re-exports wire-facing types from `shamir_query_types::validator` and
//! adds engine-internal structs: `ValidatorBinding`, `PersistedValidators`,
//! `ValidationOutcome`, `ValidatorDecodeError`, the ABI decoder, the
//! lock-free `ValidatorRegistry`, and the S3 write-path failure type
//! `ValidatorFailure`.
//!
//! ## Phase 0 additions
//!
//! - [`RecordFields`] + [`ViewFields`] + [`OwnedFields`] — by-name field access.
//! - [`RecordValidator`] + [`ValidatorCtx`] — the narrow validator role.
//! - [`NativeRecordValidator`] — Rust closure validator (by-name, zero-copy).
//! - [`WasmRecordValidator`] — WASM adapter (materialises internally).

pub mod persistence;
pub mod record_fields;
pub mod record_validator;
mod registry;

mod decode;
mod encode;
mod native_adapter;
mod native_record_validator;
mod persisted_validators;
mod query_value_conv;
mod validation_outcome;
mod validator_binding;
mod wasm_record_validator;

pub use decode::{decode_validation_result, ValidatorDecodeError};
pub use encode::{validation_to_query_value, Validation};
pub use native_adapter::{NativeValidatorAdapter, NativeValidatorFn};
pub use native_record_validator::{NativeRecordValidator, NativeValidatorFnNew};
pub use persisted_validators::PersistedValidators;
pub use query_value_conv::{inner_to_query_value, inner_to_query_value_with};
pub use record_fields::{OwnedFields, RecordFields, ViewFields};
pub use record_validator::{RecordValidator, ValidatorCtx};
pub use validation_outcome::{ValidationOutcome, ValidatorFailure};
pub use validator_binding::ValidatorBinding;
pub use wasm_record_validator::WasmRecordValidator;

pub use registry::{ValidatorRegistry, ValidatorRegistryError};

// -- re-exports from the wire crate -----------------------------------------
pub use shamir_query_types::validator::{ValidationError, WriteOp};

#[cfg(test)]
mod tests;
