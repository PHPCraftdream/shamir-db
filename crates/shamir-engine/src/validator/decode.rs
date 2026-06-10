use shamir_query_types::filter::FieldPath;
use shamir_types::types::value::QueryValue;

use super::{ValidationError, ValidationOutcome};

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
