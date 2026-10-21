//! Guest-side validation types for ShamirDB table validators.
//!
//! A validator is a WASM function bound to a table that fires before a
//! record is written. It receives the candidate record, optionally the
//! previous record (for updates), and a context, and returns a
//! [`Validation`] — an accumulating result that is either empty (accept)
//! or carries one or more [`ValidationError`]s (reject).
//!
//! The [`Validation::into_value`] method encodes the result into the
//! exact `Value` shape the engine's `decode_validation_result` expects:
//!
//! ```text
//! { "errors": [ { "field": ["address","zip"], "code": "invalid_zip" }, ... ],
//!   "stop": false }
//! ```

use crate::Value;

// ---------------------------------------------------------------------------
// Field-path helper
// ---------------------------------------------------------------------------

/// Trait for ergonomic field-path arguments.
///
/// Accepts `&str` (single segment), `&[&str]` / `[&str; N]` (path),
/// or `Vec<String>` (pre-built path).
pub trait IntoFieldPath {
    fn into_field_path(self) -> Vec<String>;
}

impl IntoFieldPath for &str {
    fn into_field_path(self) -> Vec<String> {
        vec![self.to_owned()]
    }
}

impl<const N: usize> IntoFieldPath for [&str; N] {
    fn into_field_path(self) -> Vec<String> {
        self.iter().map(|s| (*s).to_owned()).collect()
    }
}

impl IntoFieldPath for &[&str] {
    fn into_field_path(self) -> Vec<String> {
        self.iter().map(|s| (*s).to_owned()).collect()
    }
}

impl IntoFieldPath for Vec<String> {
    fn into_field_path(self) -> Vec<String> {
        self
    }
}

// ---------------------------------------------------------------------------
// ValidationError
// ---------------------------------------------------------------------------

/// One field-bound validation error.
///
/// `field` is a path into the record (e.g. `["address", "zip"]`), or
/// `None` for a record-level error. `code` is a stable, machine-readable
/// key — human-readable messages live on the frontend (i18n by code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Path into the record, or `None` for record-level.
    pub field: Option<Vec<String>>,
    /// Stable machine-readable error code.
    pub code: String,
}

// ---------------------------------------------------------------------------
// Validation (accumulating result)
// ---------------------------------------------------------------------------

/// Accumulating validation result returned by a validator function.
///
/// An empty `Validation` means **accept**; a non-empty one means
/// **reject** with the accumulated errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    errors: Vec<ValidationError>,
    stop: bool,
}

impl Validation {
    /// Accept: no errors, `stop = false`.
    pub fn accept() -> Self {
        Self {
            errors: Vec::new(),
            stop: false,
        }
    }

    /// Reject with a single field-bound error.
    pub fn reject(field: impl IntoFieldPath, code: impl Into<String>) -> Self {
        Self {
            errors: vec![ValidationError {
                field: Some(field.into_field_path()),
                code: code.into(),
            }],
            stop: false,
        }
    }

    /// Reject with a single record-level error (no field).
    pub fn record_error(code: impl Into<String>) -> Self {
        Self {
            errors: vec![ValidationError {
                field: None,
                code: code.into(),
            }],
            stop: false,
        }
    }

    /// Chainable: add a field-bound error.
    pub fn error(mut self, field: impl IntoFieldPath, code: impl Into<String>) -> Self {
        self.errors.push(ValidationError {
            field: Some(field.into_field_path()),
            code: code.into(),
        });
        self
    }

    /// Chainable: add a record-level error (no field).
    pub fn record(mut self, code: impl Into<String>) -> Self {
        self.errors.push(ValidationError {
            field: None,
            code: code.into(),
        });
        self
    }

    /// Chainable: set `stop = true` (halt remaining validators).
    pub fn stop(mut self) -> Self {
        self.stop = true;
        self
    }

    /// Returns `true` when there are no errors (i.e. the record is valid).
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Encode to the ABI `Value` the engine decodes.
    ///
    /// Shape: `Map { "errors": List<error_map>, "stop": Bool }`.
    ///
    /// Each error map: `{ "code": Str }` (record-level, field omitted)
    /// or `{ "field": List<Str>, "code": Str }` (field-bound).
    ///
    /// For record-level errors (field = None) the `"field"` key is
    /// omitted entirely — `decode_validation_result` treats a missing
    /// `"field"` key the same as `null`.
    pub fn into_value(self) -> Value {
        let error_list: Vec<Value> = self
            .errors
            .into_iter()
            .map(|e| {
                let mut entries: Vec<(String, Value)> = Vec::with_capacity(2);
                if let Some(field_path) = e.field {
                    entries.push((
                        "field".to_owned(),
                        Value::List(field_path.into_iter().map(Value::Str).collect()),
                    ));
                }
                entries.push(("code".to_owned(), Value::Str(e.code)));
                Value::Map(entries)
            })
            .collect();

        Value::Map(vec![
            ("errors".to_owned(), Value::List(error_list)),
            ("stop".to_owned(), Value::Bool(self.stop)),
        ])
    }
}

impl From<Vec<ValidationError>> for Validation {
    /// Sugar: convert a list of errors into a `Validation` with
    /// `stop = false`.
    fn from(errors: Vec<ValidationError>) -> Self {
        Self {
            errors,
            stop: false,
        }
    }
}
