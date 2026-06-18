//! [`Doc`] — record-value builder.

use shamir_query_types::filter::FilterValue;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

/// A record-value builder that produces a [`QueryValue::Map`].
///
/// Field values are either literals or computed expressions — both go
/// through [`Doc::set`], which accepts any `impl Into<FilterValue>`.
/// Literals (`i32`, `&str`, `bool`, etc.) and expressions (`col(...)`,
/// `func(...)`, `qref(...)`) all implement `Into<FilterValue>`.
///
/// For a nested map that you have already assembled as a `QueryValue`, use
/// [`Doc::set_value`].
///
/// Insertion order is preserved (backed by an `IndexMap`).
#[derive(Debug, Clone, Default)]
pub struct Doc {
    fields: TMap<String, QueryValue>,
}

/// Create an empty [`Doc`].
pub fn doc() -> Doc {
    Doc::new()
}

impl Doc {
    /// Create an empty document.
    pub fn new() -> Self {
        Self { fields: new_map() }
    }

    /// Set a field to a literal value or a computed expression.
    ///
    /// Accepts anything that converts `Into<FilterValue>`:
    /// - Literals: `i8`..`i64`, `u8`..`u32`, `f32`, `f64`, `bool`,
    ///   `&str`, `String`.
    /// - Expressions: `col(...)`, `func(...)`, `qref(...)`, `qref_all(...)`.
    ///
    /// The value is encoded via a msgpack round-trip from `FilterValue` to
    /// `QueryValue` — the two types share the same serde wire encoding.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<FilterValue>) -> Self {
        let fv: FilterValue = value.into();
        // FilterValue and QueryValue share the same serde wire encoding.
        // Round-trip via msgpack to produce a QueryValue without any
        // serde_json::Value intermediate.
        let bytes =
            rmp_serde::to_vec_named(&fv).expect("FilterValue msgpack serialization is infallible");
        let qv: QueryValue =
            rmp_serde::from_slice(&bytes).expect("FilterValue→QueryValue round-trip is infallible");
        self.fields.insert(key.into(), qv);
        self
    }

    /// Set a field to a [`QueryValue`] directly.
    ///
    /// Use this for nested maps or lists that you have already assembled as
    /// a `QueryValue` (e.g. from `mpack!({...})`).  For scalar literals and
    /// expressions, prefer [`Doc::set`].
    pub fn set_value(mut self, key: impl Into<String>, value: impl Into<QueryValue>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// Consume the builder and return the record as a `QueryValue::Map`.
    pub fn build(self) -> QueryValue {
        QueryValue::Map(self.fields)
    }
}

impl From<Doc> for QueryValue {
    fn from(doc: Doc) -> Self {
        doc.build()
    }
}
