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

pub mod schema;

mod codecs;
mod execution;
mod registry;

// Module-path facades: the pre-sweep file names stay reachable at
// `crate::validator::<name>` so no caller in any crate changes.
pub use codecs::{decode, encode, native_adapter, query_value_conv};
pub use execution::{
    native_record_validator, record_fields, record_validator, transform_spec, validation_outcome,
    wasm_record_validator,
};
pub use registry::{persisted_validators, persistence, validator_binding, validator_db};

pub use codecs::decode::{decode_validation_result, ValidatorDecodeError};
pub use codecs::encode::{validation_to_query_value, Validation};
pub use codecs::native_adapter::{NativeValidatorAdapter, NativeValidatorFn};
pub use codecs::query_value_conv::{inner_to_query_value, inner_to_query_value_with};
pub use execution::native_record_validator::{NativeRecordValidator, NativeValidatorFnNew};
pub use execution::record_fields::{OwnedFields, RecordFields, ViewFields};
pub use execution::record_validator::{RecordValidator, ValidatorCtx};
pub use execution::transform_spec::TransformSpec;
pub use execution::validation_outcome::{ValidationOutcome, ValidatorFailure};
pub use execution::wasm_record_validator::WasmRecordValidator;
pub use registry::persisted_validators::PersistedValidators;
pub use registry::validator_binding::ValidatorBinding;
pub use registry::validator_db::ValidatorDb;

pub use registry::validator_registry::{ValidatorRegistry, ValidatorRegistryError};

// -- re-exports from the wire crate -----------------------------------------
pub use shamir_query_types::validator::{ValidationError, WriteOp};

#[cfg(test)]
mod tests;
