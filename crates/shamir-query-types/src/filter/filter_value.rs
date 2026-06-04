//! FilterValue — value types supported in filters.

use serde::{Deserialize, Serialize};

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
    Binary(Vec<u8>),
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
