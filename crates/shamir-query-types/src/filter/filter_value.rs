//! FilterValue — value types supported in filters.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

use super::{Cond, FieldPath, FilterExpr, FnCall};

/// Value types supported in filters
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    // `Binary` MUST be declared before `String`: this enum is
    // `#[serde(untagged)]`, so variants are tried in declaration order and
    // the first successful match wins. The stdlib `String` Deserialize impl
    // is lenient toward raw bytes (accepts `visit_bytes`/`visit_byte_buf` as
    // a fallback), so a genuine msgpack bin8/16/32 payload whose bytes
    // happen to form valid UTF-8 (e.g. `[1, 2, 3]`) would be silently
    // captured by `String` before ever reaching `Binary` if the order were
    // left unchanged.
    //
    // Reordering alone is NOT sufficient, though: `serde_bytes`'s `Vec<u8>`
    // Deserialize (`ByteBufVisitor`) is ALSO lenient the other way — it
    // accepts `visit_str`/`visit_string` AND `visit_seq` as fallbacks (see
    // `serde_bytes::bytebuf::ByteBufVisitor`), so simply swapping the order
    // while keeping `#[serde(with = "serde_bytes")]` would make `Binary`
    // silently swallow genuine `String` and `Array` values instead (verified
    // empirically — both directions reproduced with a minimal standalone
    // enum before landing this). `de_binary_strict` below closes that hole:
    // it only accepts `visit_bytes`/`visit_byte_buf`/`visit_borrowed_bytes`
    // (the actual msgpack bin8/16/32 wire shapes), so on a `String`/`Array`
    // wire payload this variant genuinely fails and the untagged-enum trial
    // falls through to `String`/`Array` as expected. See #983 (round 2) for
    // the full investigation.
    Binary(
        #[serde(
            serialize_with = "serde_bytes::serialize",
            deserialize_with = "de_binary_strict"
        )]
        Vec<u8>,
    ),
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

/// Strict byte-buffer deserializer for `FilterValue::Binary`'s payload.
///
/// Unlike `serde_bytes::Vec<u8>` (whose `ByteBufVisitor` also accepts
/// `visit_str`/`visit_string`/`visit_seq` as lenient fallbacks — see
/// `serde_bytes::bytebuf::ByteBufVisitor`), this visitor accepts ONLY the
/// genuine byte-buffer callbacks (`visit_bytes`, `visit_byte_buf`,
/// `visit_borrowed_bytes`). Every other wire shape (string, sequence, ...)
/// is rejected with an error, which is exactly what's needed for `Binary`
/// to sit *before* `String`/`Array` in this `#[serde(untagged)]` enum
/// without swallowing them: serde's untagged-enum machinery tries each
/// variant in order and moves on to the next on any deserialize error, so a
/// genuine `String`/`Array` wire payload now correctly falls through to
/// those variants instead of being misdecoded as `Binary`. See #983
/// (round 2) for the full investigation.
fn de_binary_strict<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StrictBytesVisitor;

    impl<'de> serde::de::Visitor<'de> for StrictBytesVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a byte buffer (msgpack bin8/16/32)")
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.to_vec())
        }

        fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v)
        }

        fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.to_vec())
        }
    }

    deserializer.deserialize_bytes(StrictBytesVisitor)
}

impl FilterValue {
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

impl From<QueryValue> for FilterValue {
    /// Convert a `QueryValue` into the equivalent `FilterValue`.
    ///
    /// Conversion strategy (three-tier, no silent loss):
    /// 1. **Direct match** — literal variants (Null/Bool/Int/F64/Str/Bin/List)
    ///    are converted without serialisation via [`query_value_to_filter_value`].
    /// 2. **Msgpack fallback** — `Map` (used for expression defaults such as
    ///    `{"$fn": ...}`) and exotic numeric types (Dec/Big/Set) go through
    ///    the msgpack round-trip, which preserves the `FilterValue` serde
    ///    shape faithfully for expression variants.
    /// 3. **Last resort** — if both paths fail (malformed Map that doesn't
    ///    decode as any `FilterValue` variant), returns `FilterValue::Null`
    ///    with a `debug_assert!` so the bug is caught in dev/test runs.
    ///    Production keeps boot-resilience (no panic), but the assert catches
    ///    any regression in test suites.
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
