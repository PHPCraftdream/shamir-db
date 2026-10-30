//! Filter enum for WHERE, HAVING, UPDATE, DELETE clauses.

use serde::{Deserialize, Serialize};

use super::{FieldPath, FilterValue};
use crate::read::SelectExpr;

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
        /// Optional ranked-result cap for FTS lookups.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
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

/// One entry of the combined `Filter`/`FilterValue`/`SelectExpr` depth walk.
enum DepthNode<'a> {
    Filter(&'a Filter),
    Value(&'a FilterValue),
    SelectExpr(&'a crate::read::SelectExpr),
}

fn check_depth(depth: usize) -> Result<(), String> {
    if depth > MAX_FILTER_DEPTH {
        Err(format!("filter nesting depth exceeds {}", MAX_FILTER_DEPTH))
    } else {
        Ok(())
    }
}

pub fn check_filter_depth(filter: &Filter) -> Result<(), String> {
    check_filter_depth_at(filter, 1)
}

/// Validate nesting in an executable value surface (`FilterValue` or select expression).
pub fn check_filter_value_depth(value: &FilterValue) -> Result<(), String> {
    check_depth_tree(DepthNode::Value(value), 1)
}

pub fn check_select_expr_depth(expr: &crate::read::SelectExpr) -> Result<(), String> {
    check_depth_tree(DepthNode::SelectExpr(expr), 1)
}

fn check_depth_tree<'a>(root: DepthNode<'a>, start_depth: usize) -> Result<(), String> {
    let mut stack: Vec<(DepthNode<'a>, usize)> = vec![(root, start_depth)];
    while let Some((current, depth)) = stack.pop() {
        check_depth(depth)?;
        match current {
            DepthNode::Filter(f) => push_filter_children(f, depth, &mut stack),
            DepthNode::Value(v) => push_value_children(v, depth, &mut stack),
            DepthNode::SelectExpr(e) => push_select_expr_children(e, depth, &mut stack),
        }
    }
    Ok(())
}

/// Validate that a filter tree does not exceed `MAX_FILTER_DEPTH`, with the
/// root considered to already sit at `start_depth` levels of accumulated
/// nesting.
/// Callers checking a subtree in a larger structure should pass its accumulated
/// nesting depth as `start_depth`.
///
/// Walks both filter nesting and embedded FilterValue trees iteratively.
pub fn check_filter_depth_at(filter: &Filter, start_depth: usize) -> Result<(), String> {
    let mut stack: Vec<(DepthNode<'_>, usize)> = vec![(DepthNode::Filter(filter), start_depth)];
    while let Some((current, depth)) = stack.pop() {
        check_depth(depth)?;
        match current {
            DepthNode::Filter(f) => push_filter_children(f, depth, &mut stack),
            DepthNode::Value(v) => push_value_children(v, depth, &mut stack),
            DepthNode::SelectExpr(e) => push_select_expr_children(e, depth, &mut stack),
        }
    }
    Ok(())
}

/// Push `filter`'s immediate `Filter`/`FilterValue` children (at `depth +
/// 1`) onto the depth-walk stack. Mirrors `shamir-engine`'s
/// `compile_filter`/`resolve_filter_query` dispatch shape so every node
/// those functions can recurse into is also visited here.
fn push_filter_children<'a>(
    filter: &'a Filter,
    depth: usize,
    stack: &mut Vec<(DepthNode<'a>, usize)>,
) {
    match filter {
        Filter::And { filters } | Filter::Or { filters } => {
            for f in filters {
                stack.push((DepthNode::Filter(f), depth + 1));
            }
        }
        Filter::Not { filter } => {
            stack.push((DepthNode::Filter(filter), depth + 1));
        }
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::FieldEq { value, .. }
        | Filter::Contains { value, .. } => {
            stack.push((DepthNode::Value(value), depth + 1));
        }
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => {
            for v in values {
                stack.push((DepthNode::Value(v), depth + 1));
            }
        }
        Filter::Between { from, to, .. } => {
            stack.push((DepthNode::Value(from), depth + 1));
            stack.push((DepthNode::Value(to), depth + 1));
        }
        Filter::ValueCompare { left, right, .. } => {
            stack.push((DepthNode::Value(left), depth + 1));
            stack.push((DepthNode::Value(right), depth + 1));
        }
        Filter::Computed {
            expr_args, value, ..
        } => {
            if let Some(args) = expr_args {
                for a in args {
                    stack.push((DepthNode::Value(a), depth + 1));
                }
            }
            stack.push((DepthNode::Value(value), depth + 1));
        }
        Filter::Like { .. }
        | Filter::ILike { .. }
        | Filter::Regex { .. }
        | Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. }
        | Filter::Fts { .. }
        | Filter::VectorSimilarity { .. } => {}
    }
}

/// Push `value`'s immediate `FilterValue`/`Filter` children (at `depth +
/// 1`) onto the depth-walk stack.
fn push_value_children<'a>(
    value: &'a FilterValue,
    depth: usize,
    stack: &mut Vec<(DepthNode<'a>, usize)>,
) {
    match value {
        FilterValue::Array(items) => {
            for item in items {
                stack.push((DepthNode::Value(item), depth + 1));
            }
        }
        FilterValue::FnCall { call } => {
            for arg in call.args() {
                stack.push((DepthNode::Value(arg), depth + 1));
            }
        }
        FilterValue::Expr { expr } => {
            for arg in &expr.args {
                stack.push((DepthNode::Value(arg), depth + 1));
            }
        }
        FilterValue::Cond { cond } => {
            stack.push((DepthNode::Filter(cond.condition.as_ref()), depth + 1));
            stack.push((DepthNode::Value(&cond.then), depth + 1));
            stack.push((DepthNode::Value(&cond.or_else), depth + 1));
        }
        FilterValue::Null
        | FilterValue::Bool(_)
        | FilterValue::Int(_)
        | FilterValue::Float(_)
        | FilterValue::String(_)
        | FilterValue::Binary(_)
        | FilterValue::FieldRef { .. }
        | FilterValue::QueryRef { .. }
        | FilterValue::Param { .. } => {}
    }
}

fn push_select_expr_children<'a>(
    expr: &'a crate::read::SelectExpr,
    depth: usize,
    stack: &mut Vec<(DepthNode<'a>, usize)>,
) {
    match expr {
        SelectExpr::Add { left, right }
        | SelectExpr::Sub { left, right }
        | SelectExpr::Mul { left, right }
        | SelectExpr::Div { left, right } => {
            stack.push((DepthNode::SelectExpr(left), depth + 1));
            stack.push((DepthNode::SelectExpr(right), depth + 1));
        }
        SelectExpr::Field { .. } | SelectExpr::Literal { .. } => {}
    }
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
