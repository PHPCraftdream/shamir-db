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

    /// Value-vs-value comparison — no record/field involved. Both `left`
    /// and `right` are independently resolved (via `$query`/`$fn`/`$param`/
    /// literal, exactly like `FilterValue::Expr`) at MATCH time, then
    /// compared. This is the ONLY comparison shape meaningful inside a
    /// `when` guard (see `QueryEntry.when`), which has no per-row record
    /// to resolve a `FieldPath` against — unlike `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/
    /// `Lte`/`FieldEq` above, which stay strictly record-field-based and
    /// are used for real per-row WHERE-clause filtering.
    ValueCompare {
        left: FilterValue,
        /// Named `cmp` (not `op`) because the enclosing `Filter` enum uses
        /// `#[serde(tag = "op")]` for its own variant discriminant — a
        /// field literally named `op` would collide with that internal tag.
        cmp: ValueCompareOp,
        right: FilterValue,
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
    ///
    /// V1.1 additive fields (both optional, omitted on the wire when `None`):
    /// * `ef_search` — per-query HNSW exploration width. `None` = adapter
    ///   build-time default. Clamped server-side to `MAX_EF_SEARCH`.
    /// * `oversample` — P3 / V3.1 (leaf 3.1): candidate-widening multiplier
    ///   for filtered ANN. Consumed at the ENGINE level: the engine requests
    ///   `k′ = k × oversample` candidates, applies the residual predicate,
    ///   and retries with a doubled `k′` (up to `MAX_TOPK`) when fewer than
    ///   `k` survive. Default (when `None`) is 2×. Clamped to ≥1×.
    VectorSimilarity {
        #[serde(deserialize_with = "de_field_path")]
        field: FieldPath,
        query: Vec<f32>,
        k: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ef_search: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oversample: Option<f32>,
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

/// Comparison operator for [`Filter::ValueCompare`] — a value-vs-value
/// comparison with no record/field involved. Mirrors the 6 comparison
/// variants of `shamir-engine`'s `CompareOp` (kept as a separate type here
/// since `shamir-query-types` does not depend on `shamir-engine`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueCompareOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
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
