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

    // ── Index-accelerated operators (Phase 0 — FTS / Functional / Vector) ──

    /// Full-text search on a text field.
    /// mode: "and" (all tokens must match) or "or" (any token matches).
    Fts {
        field: FieldPath,
        query: String,
        #[serde(default = "default_fts_mode")]
        mode: String,
    },

    /// Vector similarity search (top-k nearest neighbors).
    VectorSimilarity {
        field: FieldPath,
        query: Vec<f32>,
        k: u32,
    },

    /// Comparison on a computed expression (for functional indexes).
    /// expr_op: "lower" | "upper" | "trim" | "length" | "substring" | "mod"
    /// cmp: "eq" | "lt" | "gt" | "lte" | "gte"
    Computed {
        expr_op: String,
        field: FieldPath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expr_args: Option<Vec<FilterValue>>,
        cmp: String,
        value: FilterValue,
    },
}

fn default_fts_mode() -> String {
    "and".to_string()
}
