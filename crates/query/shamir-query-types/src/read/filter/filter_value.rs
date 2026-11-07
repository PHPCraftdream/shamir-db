//! FilterValue — value types supported in filters.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

use super::{Cond, FieldPath, FilterExpr, FnCall};

#[path = "../filter_value_decoder.rs"]
mod decoder;

/// Value types supported in filters
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum FilterValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Binary(#[serde(serialize_with = "serde_bytes::serialize")] Vec<u8>),
    String(String),
    Array(Vec<FilterValue>),
    /// Reference to another field in the same document
    FieldRef {
        #[serde(rename = "$ref")]
        path: FieldPath,
    },
    /// Reference to another query's result in the same batch
    QueryRef {
        #[serde(rename = "$query")]
        alias: String,
        /// Optional path into the result
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// System function call ($fn)
    FnCall {
        #[serde(rename = "$fn")]
        call: FnCall,
    },
    /// Expression ($expr)
    Expr {
        #[serde(rename = "$expr")]
        expr: FilterExpr,
    },
    /// Conditional ($cond)
    Cond {
        #[serde(rename = "$cond")]
        cond: Box<Cond>,
    },
    /// Parameter reference — reads a named binding from the sub-batch's
    /// injected `bind` map.
    Param {
        #[serde(rename = "$param")]
        name: String,
    },
}

impl FilterValue {
    /// Convert a query value, rejecting values that cannot be decoded as a filter.
    pub fn try_from_query_value(value: QueryValue) -> Result<Self, FilterValueConversionError> {
        Self::try_from_query_value_ref(&value)
    }

    /// Decode a borrowed query value using the filter wire decoder.
    pub fn try_from_query_value_ref(
        value: &QueryValue,
    ) -> Result<Self, FilterValueConversionError> {
        if let Some(literal) = query_value_to_filter_value(value) {
            return Ok(literal);
        }
        Self::deserialize(buffered_value::BufferedValue(value))
            .map_err(|error| FilterValueConversionError::Decode(error.to_string()))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, FilterValue::Null)
    }

    /// Create a field reference from a single-segment field name
    pub fn field_ref(path: impl Into<String>) -> Self {
        FilterValue::FieldRef {
            path: vec![path.into()],
        }
    }

    /// Create a query reference (references another query's result in a batch)
    pub fn query_ref(alias: impl Into<String>) -> Self {
        FilterValue::QueryRef {
            alias: alias.into(),
            path: None,
        }
    }

    /// Create a query reference with a path
    pub fn query_ref_with_path(alias: impl Into<String>, path: impl Into<String>) -> Self {
        FilterValue::QueryRef {
            alias: alias.into(),
            path: Some(path.into()),
        }
    }
}

impl From<i8> for FilterValue {
    fn from(v: i8) -> Self {
        FilterValue::Int(i64::from(v))
    }
}

impl From<i16> for FilterValue {
    fn from(v: i16) -> Self {
        FilterValue::Int(i64::from(v))
    }
}

impl From<i32> for FilterValue {
    fn from(v: i32) -> Self {
        FilterValue::Int(i64::from(v))
    }
}

impl From<i64> for FilterValue {
    fn from(v: i64) -> Self {
        FilterValue::Int(v)
    }
}

impl From<u8> for FilterValue {
    fn from(v: u8) -> Self {
        FilterValue::Int(i64::from(v))
    }
}

impl From<u16> for FilterValue {
    fn from(v: u16) -> Self {
        FilterValue::Int(i64::from(v))
    }
}

impl From<u32> for FilterValue {
    fn from(v: u32) -> Self {
        FilterValue::Int(i64::from(v))
    }
}

impl From<f32> for FilterValue {
    fn from(v: f32) -> Self {
        FilterValue::Float(f64::from(v))
    }
}

impl From<f64> for FilterValue {
    fn from(v: f64) -> Self {
        FilterValue::Float(v)
    }
}

impl From<bool> for FilterValue {
    fn from(v: bool) -> Self {
        FilterValue::Bool(v)
    }
}

impl From<String> for FilterValue {
    fn from(v: String) -> Self {
        FilterValue::String(v)
    }
}

impl From<&str> for FilterValue {
    fn from(v: &str) -> Self {
        FilterValue::String(v.to_string())
    }
}

impl<T: Into<FilterValue>> From<Vec<T>> for FilterValue {
    fn from(v: Vec<T>) -> Self {
        FilterValue::Array(v.into_iter().map(|x| x.into()).collect())
    }
}

/// Failure to decode a query value as a filter value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterValueConversionError {
    /// The encoded query value is not a supported filter value.
    Decode(String),
}

impl std::fmt::Display for FilterValueConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(message) => write!(f, "filter value decoding failed: {message}"),
        }
    }
}

impl std::error::Error for FilterValueConversionError {}

impl From<QueryValue> for FilterValue {
    /// Legacy lossy compatibility adapter: invalid values panic in debug builds
    /// and become `Null` in release builds. Prefer `try_from_query_value`.
    fn from(qv: QueryValue) -> Self {
        // Tier 1: direct literal conversion (no allocation for common case).
        if let Some(fv) = query_value_to_filter_value(&qv) {
            return fv;
        }
        // Tier 2: msgpack round-trip for Map (expression defaults) and exotic
        // types (Dec/Big/Set) that have no direct FilterValue equivalent.
        if let Some(fv) = rmp_serde::to_vec_named(&qv)
            .ok()
            .and_then(|bytes| rmp_serde::from_slice(&bytes).ok())
        {
            return fv;
        }
        // Tier 3: genuine decode failure — developer error at build time.
        // debug_assert! makes this visible in test runs without panicking in prod.
        debug_assert!(
            false,
            "FilterValue::from(QueryValue): both direct and msgpack conversion failed \
             for {:?} — defaulting to Null",
            qv
        );
        FilterValue::Null
    }
}

/// Convert a literal [`QueryValue`] to its [`FilterValue`] equivalent.
///
/// Handles the scalar literals and `List` (recursively). Variants with no
/// `FilterValue` counterpart (`Map`, `Set`, `Dec`, `Big`) return `None`
/// — callers that need expression-default support should fall back to the
/// msgpack round-trip for those cases.
///
/// This is the mirror of [`filter_value_to_query_value`]: for every literal
/// variant the round-trip is lossless.
///
/// Note: `QueryValue::F64` maps to `FilterValue::Float` (the f64 wrapper).
pub fn query_value_to_filter_value(qv: &QueryValue) -> Option<FilterValue> {
    match qv {
        QueryValue::Null => Some(FilterValue::Null),
        QueryValue::Bool(b) => Some(FilterValue::Bool(*b)),
        QueryValue::Int(i) => Some(FilterValue::Int(*i)),
        QueryValue::F64(f) => Some(FilterValue::Float(*f)),
        QueryValue::Str(s) => Some(FilterValue::String(s.clone())),
        QueryValue::Bin(b) => Some(FilterValue::Binary(b.clone())),
        QueryValue::List(items) => {
            let fv_items: Option<Vec<FilterValue>> =
                items.iter().map(query_value_to_filter_value).collect();
            fv_items.map(FilterValue::Array)
        }
        // Map, Set, Dec, Big — no direct FilterValue equivalent.
        // Callers should use the msgpack round-trip for Map (expression defaults).
        _ => None,
    }
}

/// Convert a literal [`FilterValue`] to its [`QueryValue`] equivalent.
///
/// Handles the scalar literals and `Array` (recursively). Expression
/// variants (`FieldRef`, `QueryRef`, `FnCall`, `Expr`, `Cond`, `Param`)
/// return `None` — callers should use the msgpack round-trip for those.
///
/// This is a copy of the `pub(crate)` function in `shamir-engine`'s
/// `schema_validator` module, lifted here so that `shamir-db` (which
/// depends on `shamir-query-types` but not on engine internals) can use it
/// directly on the write path without a cross-crate private-visibility reach.
pub fn filter_value_to_query_value(fv: &FilterValue) -> Option<QueryValue> {
    match fv {
        FilterValue::Null => Some(QueryValue::Null),
        FilterValue::Bool(b) => Some(QueryValue::Bool(*b)),
        FilterValue::Int(i) => Some(QueryValue::Int(*i)),
        FilterValue::Float(f) => Some(QueryValue::F64(*f)),
        FilterValue::String(s) => Some(QueryValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(QueryValue::Bin(b.clone())),
        FilterValue::Array(items) => {
            let qv_items: Option<Vec<QueryValue>> =
                items.iter().map(filter_value_to_query_value).collect();
            qv_items.map(QueryValue::List)
        }
        // Expression variants — not literals.
        _ => None,
    }
}

mod buffered_value {

    use serde::de::value::{Error, MapAccessDeserializer, MapDeserializer, SeqDeserializer};
    use serde::de::{Deserializer, IntoDeserializer, Visitor};
    use shamir_types::types::value::QueryValue;

    pub(super) struct BufferedValue<'a>(pub(super) &'a QueryValue);

    impl<'de> IntoDeserializer<'de, Error> for BufferedValue<'de> {
        type Deserializer = Self;
        fn into_deserializer(self) -> Self {
            self
        }
    }

    impl<'de> Deserializer<'de> for BufferedValue<'de> {
        type Error = Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self.0 {
                QueryValue::Null => visitor.visit_unit(),
                QueryValue::Bool(v) => visitor.visit_bool(*v),
                QueryValue::Int(v) => visitor.visit_i64(*v),
                QueryValue::F64(v) => visitor.visit_f64(*v),
                QueryValue::Dec(v) => visitor.visit_string(v.to_string()),
                QueryValue::Big(v) => visitor.visit_string(v.to_string()),
                QueryValue::Str(v) => visitor.visit_borrowed_str(v),
                QueryValue::Bin(v) => visitor.visit_borrowed_bytes(v),
                QueryValue::List(v) => {
                    let mut seq = SeqDeserializer::<_, Error>::new(v.iter().map(BufferedValue));
                    let result = visitor.visit_seq(&mut seq)?;
                    seq.end()?;
                    Ok(result)
                }
                QueryValue::Set(v) => {
                    let mut seq = SeqDeserializer::<_, Error>::new(v.iter().map(BufferedValue));
                    let result = visitor.visit_seq(&mut seq)?;
                    seq.end()?;
                    Ok(result)
                }
                QueryValue::Map(v) => {
                    let mut map = MapDeserializer::<_, Error>::new(
                        v.iter().map(|(k, v)| (k.as_str(), BufferedValue(v))),
                    );
                    let result = visitor.visit_map(&mut map)?;
                    map.end()?;
                    Ok(result)
                }
            }
        }

        fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self.0 {
                QueryValue::Null => visitor.visit_none(),
                _ => visitor.visit_some(self),
            }
        }
        fn deserialize_newtype_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            visitor: V,
        ) -> Result<V::Value, Error> {
            visitor.visit_newtype_struct(self)
        }
        fn deserialize_enum<V: Visitor<'de>>(
            self,
            _: &'static str,
            _: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Error> {
            match self.0 {
                QueryValue::Str(value) => visitor.visit_enum(value.as_str().into_deserializer()),
                QueryValue::Int(value) => visitor.visit_enum(
                    u32::try_from(*value)
                        .map_err(serde::de::Error::custom)?
                        .into_deserializer(),
                ),
                QueryValue::Map(value) if value.len() == 1 => visitor.visit_enum(
                    MapAccessDeserializer::new(MapDeserializer::<_, Error>::new(
                        value.iter().map(|(k, v)| (k.as_str(), BufferedValue(v))),
                    )),
                ),
                _ => Err(serde::de::Error::custom(
                    "expected enum identifier or single-entry map",
                )),
            }
        }
        fn is_human_readable(&self) -> bool {
            false
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
        }
    }
}
