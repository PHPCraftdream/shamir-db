//! Filter enum for WHERE, HAVING, UPDATE, DELETE clauses.

use serde::{Deserialize, Serialize};

use super::{FieldPath, FilterValue};

/// Maximum nesting depth for filter trees. Deeply-nested `$cond`/`not`/`and`/`or`
/// beyond this cap will be rejected to prevent stack overflow post-handshake.
pub const MAX_FILTER_DEPTH: usize = 64;

/// A complete filter expression (WHERE/HAVING)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Filter {
    // Comparison operators
    Eq {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },
    Ne {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },
    Gt {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },
    Gte {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },
    Lt {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },
    Lte {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },

    // Pattern matching
    Like {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        pattern: String,
    },
    ILike {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        pattern: String,
    },
    Regex {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        pattern: String,
    },

    // Null checks
    IsNull {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
    },
    IsNotNull {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
    },

    // Array/containment operators
    In {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    NotIn {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    Contains {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },
    ContainsAny {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    ContainsAll {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        values: Vec<FilterValue>,
    },

    // Range
    Between {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        from: FilterValue,
        to: FilterValue,
    },

    // Existence
    Exists {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
    },
    NotExists {
        #[serde(deserialize_with = "de_field_path")]
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
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        value: FilterValue,
    },

    // ── Index-accelerated operators (Phase 0 — FTS / Functional / Vector) ──
    /// Full-text search on a text field.
    /// mode: "and" (all tokens must match) or "or" (any token matches).
    Fts {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        query: String,
        #[serde(default = "default_fts_mode")]
        mode: String,
    },

    /// Vector similarity search (top-k nearest neighbors).
    VectorSimilarity {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        query: Vec<f32>,
        k: u32,
    },

    /// Comparison on a computed expression (for functional indexes).
    /// expr_op: "lower" | "upper" | "trim" | "length" | "substring" | "mod"
    /// cmp: "eq" | "lt" | "gt" | "lte" | "gte"
    Computed {
        expr_op: String,
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expr_args: Option<Vec<FilterValue>>,
        cmp: String,
        value: FilterValue,
    },
}

/// Validate that a filter tree does not exceed `MAX_FILTER_DEPTH`.
/// Uses an explicit stack (iterative, no unbounded recursion).
/// Returns `Ok(())` if the tree is within bounds.
pub fn check_filter_depth(filter: &Filter) -> Result<(), String> {
    let mut stack: Vec<(&Filter, usize)> = vec![(filter, 1)];
    while let Some((current, depth)) = stack.pop() {
        if depth > MAX_FILTER_DEPTH {
            return Err(format!("filter nesting depth exceeds {}", MAX_FILTER_DEPTH));
        }
        match current {
            Filter::And { filters } | Filter::Or { filters } => {
                for f in filters {
                    stack.push((f, depth + 1));
                }
            }
            Filter::Not { filter } => {
                stack.push((filter, depth + 1));
            }
            _ => {}
        }
    }
    Ok(())
}

fn default_fts_mode() -> String {
    "and".to_string()
}

/// Deserialize a [`FieldPath`] from EITHER a single string (a top-level
/// field, e.g. `"id"`) OR an array of segments (a nested document path,
/// e.g. `["address", "city"]` → `record.address.city`).
///
/// This keeps the common single-field case ergonomic — `"field": "id"` —
/// while still supporting nested paths via an array. Serialization always
/// emits the canonical array form.
fn de_field_path<'de, D>(deserializer: D) -> Result<FieldPath, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrSeq {
        One(String),
        Many(Vec<String>),
    }
    Ok(match StringOrSeq::deserialize(deserializer)? {
        StringOrSeq::One(s) => vec![s],
        StringOrSeq::Many(v) => v,
    })
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
        let json =
            r#"{"op":"computed","expr_op":"lower","field":["email"],"cmp":"eq","value":"alice"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        match &f {
            Filter::Computed {
                expr_op,
                field,
                cmp,
                value,
                expr_args,
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

    #[test]
    fn field_accepts_bare_string_as_single_segment() {
        // Ergonomic: a top-level field may be written as a bare string.
        let json = r#"{"op":"eq","field":"id","value":"user:42"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        match &f {
            Filter::Eq { field, .. } => assert_eq!(field, &["id"]),
            _ => panic!("expected Eq"),
        }
        // Serializes back to the canonical array form, which re-parses.
        let back = serde_json::to_string(&f).unwrap();
        assert!(back.contains(r#""field":["id"]"#), "serialized: {back}");
        let f2: Filter = serde_json::from_str(&back).unwrap();
        assert_eq!(f, f2);
    }

    #[test]
    fn field_accepts_nested_path_array() {
        let json = r#"{"op":"eq","field":["address","city"],"value":"NY"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        match &f {
            Filter::Eq { field, .. } => assert_eq!(field, &["address", "city"]),
            _ => panic!("expected Eq"),
        }
    }

    #[test]
    fn deep_filter_rejected_by_depth_check() {
        use super::check_filter_depth;
        // Build a deeply nested Not chain: Not(Not(Not(...(Eq)...)))
        let mut f = Filter::Eq {
            field: vec!["id".into()],
            value: crate::filter::FilterValue::String("x".into()),
        };
        for _ in 0..100 {
            f = Filter::Not {
                filter: Box::new(f),
            };
        }
        let result = check_filter_depth(&f);
        assert!(result.is_err(), "deeply nested filter should be rejected");
    }

    #[test]
    fn normal_filter_depth_passes() {
        use super::check_filter_depth;
        let f = Filter::And {
            filters: vec![
                Filter::Eq {
                    field: vec!["a".into()],
                    value: crate::filter::FilterValue::String("b".into()),
                },
                Filter::Not {
                    filter: Box::new(Filter::Eq {
                        field: vec!["c".into()],
                        value: crate::filter::FilterValue::Int(1),
                    }),
                },
            ],
        };
        assert!(check_filter_depth(&f).is_ok());
    }
}
