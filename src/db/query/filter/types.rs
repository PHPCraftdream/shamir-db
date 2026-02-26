//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.
//!
//! Supports comparison, logical, and array operators.

use serde::{Deserialize, Serialize};

/// Field path (e.g., "user.email" or "tags")
pub type FieldPath = String;

/// A complete filter expression (WHERE/HAVING)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Filter {
    // Comparison operators
    Eq {
        field: FieldPath,
        value: FilterValue,
    },
    Ne {
        field: FieldPath,
        value: FilterValue,
    },
    Gt {
        field: FieldPath,
        value: FilterValue,
    },
    Gte {
        field: FieldPath,
        value: FilterValue,
    },
    Lt {
        field: FieldPath,
        value: FilterValue,
    },
    Lte {
        field: FieldPath,
        value: FilterValue,
    },

    // Pattern matching
    Like {
        field: FieldPath,
        pattern: String,
    },
    ILike {
        field: FieldPath,
        pattern: String,
    },
    Regex {
        field: FieldPath,
        pattern: String,
    },

    // Null checks
    IsNull {
        field: FieldPath,
    },
    IsNotNull {
        field: FieldPath,
    },

    // Array/containment operators
    In {
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    NotIn {
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    Contains {
        field: FieldPath,
        value: FilterValue,
    },
    ContainsAny {
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    ContainsAll {
        field: FieldPath,
        values: Vec<FilterValue>,
    },

    // Range
    Between {
        field: FieldPath,
        from: FilterValue,
        to: FilterValue,
    },

    // Existence
    Exists {
        field: FieldPath,
    },
    NotExists {
        field: FieldPath,
    },

    // Logical operators
    And {
        filters: Vec<Filter>,
    },
    Or {
        filters: Vec<Filter>,
    },
    Not {
        filter: Box<Filter>,
    },

    // Shortcut: field equals value
    #[serde(rename = "field")]
    FieldEq {
        field: FieldPath,
        value: FilterValue,
    },
}

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
}

impl FilterValue {
    pub fn is_null(&self) -> bool {
        matches!(self, FilterValue::Null)
    }
}

impl From<i64> for FilterValue {
    fn from(v: i64) -> Self {
        FilterValue::Int(v)
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
