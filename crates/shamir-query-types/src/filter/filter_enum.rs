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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_serde_round_trip() {
        let json = r#"{"op":"fts","field":["body"],"query":"hello world","mode":"and"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        match &f {
            Filter::Fts { field, query, mode } => {
                assert_eq!(field, &["body"]);
                assert_eq!(query, "hello world");
                assert_eq!(mode, "and");
            }
            _ => panic!("expected Fts"),
        }
        let back = serde_json::to_string(&f).unwrap();
        let f2: Filter = serde_json::from_str(&back).unwrap();
        assert_eq!(f, f2);
    }

    #[test]
    fn fts_default_mode_is_and() {
        let json = r#"{"op":"fts","field":["body"],"query":"test"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        match &f {
            Filter::Fts { mode, .. } => assert_eq!(mode, "and"),
            _ => panic!("expected Fts"),
        }
    }

    #[test]
    fn vector_similarity_serde_round_trip() {
        let json = r#"{"op":"vector_similarity","field":["emb"],"query":[1.0,0.0,0.5],"k":10}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        match &f {
            Filter::VectorSimilarity { field, query, k } => {
                assert_eq!(field, &["emb"]);
                assert_eq!(query.len(), 3);
                assert_eq!(*k, 10);
            }
            _ => panic!("expected VectorSimilarity"),
        }
        let back = serde_json::to_string(&f).unwrap();
        let f2: Filter = serde_json::from_str(&back).unwrap();
        assert_eq!(f, f2);
    }

    #[test]
    fn computed_serde_round_trip() {
        let json = r#"{"op":"computed","expr_op":"lower","field":["email"],"cmp":"eq","value":"alice"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        match &f {
            Filter::Computed {
                expr_op, field, cmp, value, expr_args,
            } => {
                assert_eq!(expr_op, "lower");
                assert_eq!(field, &["email"]);
                assert_eq!(cmp, "eq");
                assert_eq!(value, &FilterValue::String("alice".into()));
                assert!(expr_args.is_none());
            }
            _ => panic!("expected Computed"),
        }
        let back = serde_json::to_string(&f).unwrap();
        let f2: Filter = serde_json::from_str(&back).unwrap();
        assert_eq!(f, f2);
    }

    #[test]
    fn existing_eq_still_works() {
        let json = r#"{"op":"eq","field":["age"],"value":30}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        assert!(matches!(f, Filter::Eq { .. }));
    }
}
