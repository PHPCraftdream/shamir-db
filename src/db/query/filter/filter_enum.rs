//! Filter enum for WHERE, HAVING, UPDATE, DELETE clauses.

use serde::{Deserialize, Serialize};

use super::{FieldPath, FilterValue};

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
