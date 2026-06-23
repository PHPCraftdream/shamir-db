//! Host-side validation accumulator + [`QueryValue`] encoder.
//!
//! Mirrors the SDK guest-side `Validation` API
//! (`shamir_sdk::validation::Validation`) but operates on [`QueryValue`]
//! rather than the SDK's msgpack-wire `Value`. The encoder output is the
//! Map form `{ "errors": List<err>, "stop": Bool }` that
//! [`decode_validation_result`] round-trips (see Seam 2.2 of the Phase 0
//! findings for the exact contract).
//!
//! Native validators built via [`crate::validator`] return this type; the
//! adapter encodes it to `QueryValue` before handing it back to the engine's
//! invocation path.

use shamir_query_types::validator::ValidationError;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

/// Host-side validation result accumulator.
///
/// `errors` is a list of field-bound error codes; `stop` requests the engine
/// to skip remaining validators for the current write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validation {
    /// Field-bound errors. An empty list means the write is accepted.
    pub errors: Vec<ValidationError>,
    /// If `true`, the engine stops evaluating further validators.
    pub stop: bool,
}

impl Validation {
    /// Create an empty (accepting) validation result.
    pub fn accept() -> Self {
        Self::default()
    }

    /// Create a rejected result with a single record-level error.
    pub fn reject(code: impl Into<String>) -> Self {
        Self {
            errors: vec![ValidationError {
                field: None,
                code: code.into(),
            }],
            stop: false,
        }
    }

    /// Add a record-level error (no field path).
    pub fn error(&mut self, code: impl Into<String>) -> &mut Self {
        self.errors.push(ValidationError {
            field: None,
            code: code.into(),
        });
        self
    }

    /// Add a field-bound error.
    pub fn field_error(&mut self, field: Vec<String>, code: impl Into<String>) -> &mut Self {
        self.errors.push(ValidationError {
            field: Some(field),
            code: code.into(),
        });
        self
    }

    /// Request the engine to stop after this validator.
    pub fn stop(&mut self) -> &mut Self {
        self.stop = true;
        self
    }

    /// Whether this result has no errors (the write is accepted).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Encode a [`Validation`] to the [`QueryValue`] Map form consumed by
/// [`decode_validation_result`].
///
/// Produces `{ "errors": List<err>, "stop": Bool }` where each `<err>` is
/// either `Map { "code": Str, "field": List<Str> }` (when `field` is `Some`)
/// or `Map { "code": Str }` (when `field` is `None`).
pub fn validation_to_query_value(v: &Validation) -> QueryValue {
    let error_list: Vec<QueryValue> = v
        .errors
        .iter()
        .map(|e| {
            let mut entries = new_map();
            if let Some(field_path) = &e.field {
                entries.insert(
                    "field".to_string(),
                    QueryValue::List(
                        field_path
                            .iter()
                            .map(|s| QueryValue::Str(s.clone()))
                            .collect(),
                    ),
                );
            }
            entries.insert("code".to_string(), QueryValue::Str(e.code.clone()));
            QueryValue::Map(entries)
        })
        .collect();

    let mut root = new_map();
    root.insert("errors".to_string(), QueryValue::List(error_list));
    root.insert("stop".to_string(), QueryValue::Bool(v.stop));
    QueryValue::Map(root)
}
