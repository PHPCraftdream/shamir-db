//! [`Doc`] — record-value builder.

use serde_json::{Map, Value};
use shamir_query_types::filter::FilterValue;

/// A record-value builder that produces a [`serde_json::Value::Object`].
///
/// Field values are either literals or computed expressions — both go
/// through [`Doc::set`], which accepts any `impl Into<FilterValue>`.
/// Literals (`i32`, `&str`, `bool`, etc.) and expressions (`col(...)`,
/// `func(...)`, `qref(...)`) all implement `Into<FilterValue>`.
///
/// For the rare case of a nested literal JSON object/array (which
/// `FilterValue` cannot represent — it has no Map variant), use
/// [`Doc::set_json`].
///
/// Insertion order is preserved when the `serde_json` crate has
/// `preserve_order` enabled; otherwise iteration order is unspecified.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    fields: Map<String, Value>,
}

/// Create an empty [`Doc`].
pub fn doc() -> Doc {
    Doc::new()
}

impl Doc {
    /// Create an empty document.
    pub fn new() -> Self {
        Self { fields: Map::new() }
    }

    /// Set a field to a literal value or a computed expression.
    ///
    /// Accepts anything that converts `Into<FilterValue>`:
    /// - Literals: `i8`..`i64`, `u8`..`u32`, `f32`, `f64`, `bool`,
    ///   `&str`, `String`.
    /// - Expressions: `col(...)`, `func(...)`, `qref(...)`, `qref_all(...)`.
    ///
    /// The value is serialized to `serde_json::Value` internally.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<FilterValue>) -> Self {
        let fv: FilterValue = value.into();
        let json = serde_json::to_value(fv).expect("FilterValue serialization is infallible");
        self.fields.insert(key.into(), json);
        self
    }

    /// Set a field to a raw JSON value.
    ///
    /// Use this for nested literal objects or arrays that `FilterValue`
    /// cannot represent (it has no Map variant). For everything else,
    /// prefer [`Doc::set`].
    pub fn set_json(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// Consume the builder and return the JSON object.
    pub fn build(self) -> Value {
        Value::Object(self.fields)
    }
}

impl From<Doc> for Value {
    fn from(doc: Doc) -> Self {
        doc.build()
    }
}
