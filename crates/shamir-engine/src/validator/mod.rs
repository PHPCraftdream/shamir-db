//! Validator types for the engine layer.
//!
//! Re-exports wire-facing types from `shamir_query_types::validator` and
//! adds engine-internal structs: `ValidatorBinding`, `PersistedValidators`,
//! `ValidationOutcome`, `ValidatorDecodeError`, the ABI decoder, the
//! lock-free `ValidatorRegistry`, and the S3 write-path failure type
//! `ValidatorFailure`.

pub mod persistence;
mod registry;

use shamir_query_types::filter::FieldPath;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue, Value};
use smallvec::SmallVec;

use serde::{Deserialize, Serialize};

pub use registry::{ValidatorRegistry, ValidatorRegistryError};

// -- re-exports from the wire crate -----------------------------------------
pub use shamir_query_types::validator::{ValidationError, WriteOp};

/// A single validator-to-table binding stored in the info-twin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorBinding {
    /// Catalogue record `_id` of the validator (resolved from name at
    /// bind time).
    pub validator_id: RecordId,
    /// Which write operations this validator fires on.
    pub ops: SmallVec<[WriteOp; 4]>,
    /// Execution priority: lower = earlier. Range `[1000, 9999]`.
    pub priority: u16,
}

/// Persisted per-table validator bindings (mirrors `PersistedIndexes`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedValidators {
    pub bindings: Vec<ValidatorBinding>,
}

/// Aggregated result of running one validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationOutcome {
    /// Field-bound errors returned by the validator.
    pub errors: Vec<ValidationError>,
    /// If `true`, the engine should skip remaining validators.
    pub stop: bool,
}

// -- S3: write-path failure types + InnerValue→QueryValue conversion ----

/// Failure modes for the S3 validator pass on the write path.
///
/// - `Failed` — one or more validators returned field-bound errors.
/// - `Missing` — a binding references a validator id that is not
///   registered (fail-closed: operator/deploy fault).
/// - `Invocation` — the validator trapped, returned an undecodable
///   result, or produced any other runtime error (fail-closed).
#[derive(Debug)]
pub enum ValidatorFailure {
    /// The validator(s) produced field-bound errors — the write is
    /// rejected with a structured error list.
    Failed(Vec<ValidationError>),
    /// A binding references a validator id that is not in the registry.
    Missing { id: RecordId },
    /// The validator invocation failed (WASM trap, undecodable return,
    /// function error).
    Invocation { id: RecordId, reason: String },
}

impl std::fmt::Display for ValidatorFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(errors) => {
                write!(f, "validator rejected: ")?;
                for (i, e) in errors.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    if let Some(ref field) = e.field {
                        write!(f, "{}: {}", field.join("."), e.code)?;
                    } else {
                        write!(f, "{}", e.code)?;
                    }
                }
                Ok(())
            }
            Self::Missing { id } => write!(f, "validator {id} not found in registry (fail-closed)"),
            Self::Invocation { id, reason } => {
                write!(f, "validator {id} invocation failed: {reason}")
            }
        }
    }
}

/// Convert an [`InnerValue`] (interned keys) to a [`QueryValue`] (string
/// keys) using the given interner. Used by `run_validators` to build the
/// `record` / `old_record` params that a validator receives.
///
/// This is a lightweight recursive conversion that avoids the JSON
/// round-trip of `inner_to_json_value` + `json_value_to_inner`.
pub fn inner_to_query_value(value: &InnerValue, interner: &Interner) -> Result<QueryValue, String> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Bool(b) => Ok(Value::Bool(*b)),
        Value::Int(i) => Ok(Value::Int(*i)),
        Value::F64(f) => Ok(Value::F64(*f)),
        Value::Dec(d) => Ok(Value::Dec(*d)),
        Value::Big(b) => Ok(Value::Big(b.clone())),
        Value::Str(s) => Ok(Value::Str(s.clone())),
        Value::Bin(b) => Ok(Value::Bin(b.clone())),
        Value::List(l) => {
            let mut out = Vec::with_capacity(l.len());
            for v in l {
                out.push(inner_to_query_value(v, interner)?);
            }
            Ok(Value::List(out))
        }
        Value::Set(s) => {
            let mut out = shamir_types::types::common::new_set();
            for v in s {
                out.insert(inner_to_query_value(v, interner)?);
            }
            Ok(Value::Set(out))
        }
        Value::Map(m) => {
            let mut out = shamir_types::types::common::new_map();
            for (k, v) in m {
                let key_str = deintern(interner, k)?;
                out.insert(key_str, inner_to_query_value(v, interner)?);
            }
            Ok(Value::Map(out))
        }
    }
}

/// Helper: resolve an interned key to its string form.
fn deintern(interner: &Interner, key: &InternerKey) -> Result<String, String> {
    interner
        .with_str(key, |s| s.to_string())
        .ok_or_else(|| format!("interned key {:?} not found", key))
}

/// Errors when decoding a validator's `QueryValue` return into
/// `ValidationOutcome`.
#[derive(Debug, thiserror::Error)]
pub enum ValidatorDecodeError {
    #[error("unexpected root type: expected null, list, or map with \"errors\" key")]
    UnexpectedRootType,
    #[error("error item is not a string or map")]
    BadItemType,
    #[error("error map is missing required \"code\" key")]
    MissingCode,
    #[error("\"code\" value is not a string")]
    NonStringCode,
    #[error("\"field\" value is not a list of strings or null")]
    BadFieldType,
}

/// Decode a validator's `QueryValue` return into a `ValidationOutcome`
/// following the ABI convention described in `VALIDATORS.md`.
///
/// - `Value::Null` -> valid (empty errors, `stop = false`).
/// - `Value::List` -> each item decoded as a `ValidationError`; `stop = false`.
/// - `Value::Map` with `"errors"` key (a list) and optional `"stop"` bool.
/// - Anything else -> `Err(ValidatorDecodeError)`.
pub fn decode_validation_result(v: &QueryValue) -> Result<ValidationOutcome, ValidatorDecodeError> {
    match v {
        // null => valid
        QueryValue::Null => Ok(ValidationOutcome {
            errors: Vec::new(),
            stop: false,
        }),

        // bare list => errors, stop=false
        QueryValue::List(items) => {
            let errors = decode_error_list(items)?;
            Ok(ValidationOutcome {
                errors,
                stop: false,
            })
        }

        // map with "errors" key
        QueryValue::Map(map) => {
            let errors_val = map
                .get("errors")
                .ok_or(ValidatorDecodeError::UnexpectedRootType)?;

            let items = match errors_val {
                QueryValue::List(items) => items,
                _ => return Err(ValidatorDecodeError::UnexpectedRootType),
            };

            let errors = decode_error_list(items)?;

            let stop = match map.get("stop") {
                Some(QueryValue::Bool(b)) => *b,
                None => false,
                _ => false, // non-bool "stop" treated as default false
            };

            Ok(ValidationOutcome { errors, stop })
        }

        _ => Err(ValidatorDecodeError::UnexpectedRootType),
    }
}

/// Decode a list of `QueryValue` items into `Vec<ValidationError>`.
fn decode_error_list(items: &[QueryValue]) -> Result<Vec<ValidationError>, ValidatorDecodeError> {
    let mut errors = Vec::with_capacity(items.len());
    for item in items {
        errors.push(decode_single_error(item)?);
    }
    Ok(errors)
}

/// Decode one error item: either a bare string (record-level error) or
/// a map `{ "field": [..] | null, "code": "str" }`.
fn decode_single_error(item: &QueryValue) -> Result<ValidationError, ValidatorDecodeError> {
    match item {
        // bare string => record-level error with that string as the code
        QueryValue::Str(code) => Ok(ValidationError {
            field: None,
            code: code.clone(),
        }),

        QueryValue::Map(map) => {
            // "code" is required and must be a string
            let code = match map.get("code") {
                Some(QueryValue::Str(s)) => s.clone(),
                Some(_) => return Err(ValidatorDecodeError::NonStringCode),
                None => return Err(ValidatorDecodeError::MissingCode),
            };

            // "field" is optional: null/absent => None; list of strings => Some
            let field = match map.get("field") {
                None | Some(QueryValue::Null) => None,
                Some(QueryValue::List(parts)) => {
                    let mut path: FieldPath = Vec::with_capacity(parts.len());
                    for part in parts {
                        match part {
                            QueryValue::Str(s) => path.push(s.clone()),
                            _ => return Err(ValidatorDecodeError::BadFieldType),
                        }
                    }
                    Some(path)
                }
                _ => return Err(ValidatorDecodeError::BadFieldType),
            };

            Ok(ValidationError { field, code })
        }

        _ => Err(ValidatorDecodeError::BadItemType),
    }
}

#[cfg(test)]
mod tests;
