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
    String(String),
    Binary(#[serde(with = "serde_bytes")] Vec<u8>),
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
