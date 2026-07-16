use shamir_collections::TSet;

use super::filter_node::{CompareOp, FilterNode};
use super::fts::like_pattern_to_regex;
use super::resolve::{filter_value_to_query, intern_field_path, intern_field_path_compact};
use crate::query::filter::{Filter, FilterValue};
use regex::Regex;
use shamir_query_types::filter::ValueCompareOp;
use shamir_types::core::interner::Interner;

/// Convert the DTO-level `ValueCompareOp` (defined in `shamir-query-types`,
/// which does not depend on `shamir-engine`) into the engine's `CompareOp`
/// used by the compiled `FilterNode` tree.
fn value_compare_op_to_compare_op(op: ValueCompareOp) -> CompareOp {
    match op {
        ValueCompareOp::Eq => CompareOp::Eq,
        ValueCompareOp::Ne => CompareOp::Ne,
        ValueCompareOp::Gt => CompareOp::Gt,
        ValueCompareOp::Gte => CompareOp::Gte,
        ValueCompareOp::Lt => CompareOp::Lt,
        ValueCompareOp::Lte => CompareOp::Lte,
    }
}

/// Compile a Filter AST into a tree of `FilterNode` (static dispatch).
///
/// Field paths are resolved via the interner at compile time. If a field path
/// cannot be interned (field doesn't exist), the node folds to True/False.
pub fn compile_filter(filter: &Filter, interner: &Interner) -> FilterNode {
    match filter {
        Filter::Eq { field, value } => compile_compare(field, value, CompareOp::Eq, interner),
        Filter::Ne { field, value } => compile_compare(field, value, CompareOp::Ne, interner),
        Filter::Gt { field, value } => compile_compare(field, value, CompareOp::Gt, interner),
        Filter::Gte { field, value } => compile_compare(field, value, CompareOp::Gte, interner),
        Filter::Lt { field, value } => compile_compare(field, value, CompareOp::Lt, interner),
        Filter::Lte { field, value } => compile_compare(field, value, CompareOp::Lte, interner),
        Filter::FieldEq { field, value } => compile_compare(field, value, CompareOp::Eq, interner),

        // Value-vs-value comparison (#651 fix) — no field path to resolve,
        // so no compile-time folding: both sides are resolved at MATCH
        // time via `resolve_filter_query` (their `$query` refs vary per
        // call). See `FilterNode::ValueCompare::matches` for the eval.
        Filter::ValueCompare { left, cmp, right } => FilterNode::ValueCompare {
            left: left.clone(),
            op: value_compare_op_to_compare_op(*cmp),
            right: right.clone(),
        },

        Filter::And { filters } => FilterNode::And(
            filters
                .iter()
                .map(|f| compile_filter(f, interner))
                .collect(),
        ),
        Filter::Or { filters } => FilterNode::Or(
            filters
                .iter()
                .map(|f| compile_filter(f, interner))
                .collect(),
        ),
        Filter::Not { filter } => FilterNode::Not(Box::new(compile_filter(filter, interner))),

        Filter::IsNull { field } => match intern_field_path_compact(field, interner) {
            Some(path) => FilterNode::IsNull { field_path: path },
            None => FilterNode::True,
        },
        Filter::IsNotNull { field } => match intern_field_path_compact(field, interner) {
            Some(path) => FilterNode::IsNotNull { field_path: path },
            None => FilterNode::False,
        },

        Filter::In { field, values } => match intern_field_path_compact(field, interner) {
            Some(path) => compile_in_node(path, values, false),
            None => FilterNode::False,
        },
        Filter::NotIn { field, values } => match intern_field_path_compact(field, interner) {
            Some(path) => compile_in_node(path, values, true),
            None => FilterNode::True,
        },

        Filter::Like { field, pattern } => match intern_field_path_compact(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, false) {
                Some(regex) => FilterNode::Like {
                    field_path: path,
                    regex,
                },
                None => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::ILike { field, pattern } => match intern_field_path_compact(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, true) {
                Some(regex) => FilterNode::Like {
                    field_path: path,
                    regex,
                },
                None => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::Regex { field, pattern } => match intern_field_path_compact(field, interner) {
            Some(path) => match Regex::new(pattern) {
                Ok(regex) => FilterNode::Regex {
                    field_path: path,
                    regex,
                },
                Err(_) => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::Contains { field, value } => match intern_field_path_compact(field, interner) {
            Some(path) => FilterNode::Contains {
                field_path: path,
                pre_resolved: filter_value_to_query(value),
                value: value.clone(),
            },
            None => FilterNode::False,
        },
        Filter::ContainsAny { field, values } => match intern_field_path_compact(field, interner) {
            Some(path) => compile_contains_any_node(path, values),
            None => FilterNode::False,
        },
        Filter::ContainsAll { field, values } => match intern_field_path_compact(field, interner) {
            Some(path) => compile_contains_all_node(path, values),
            None => FilterNode::False,
        },
        Filter::Between { field, from, to } => match intern_field_path_compact(field, interner) {
            Some(path) => FilterNode::Between {
                field_path: path,
                pre_from: filter_value_to_query(from),
                pre_to: filter_value_to_query(to),
                from: from.clone(),
                to: to.clone(),
            },
            None => FilterNode::False,
        },
        Filter::Exists { field } => match intern_field_path_compact(field, interner) {
            Some(path) => FilterNode::Exists { field_path: path },
            None => FilterNode::False,
        },
        Filter::NotExists { field } => match intern_field_path_compact(field, interner) {
            Some(path) => FilterNode::NotExists { field_path: path },
            None => FilterNode::True,
        },

        // Vector similarity cannot be brute-forced per-record
        // (would be O(n×dim) without an index). Planner must handle.
        Filter::VectorSimilarity { .. } => FilterNode::True,

        // FTS brute-force fallback (when no FTS index exists).
        Filter::Fts { field, query, mode } => match intern_field_path_compact(field, interner) {
            Some(path) => FilterNode::FtsMatch {
                field_path: path,
                query_tokens: query.split_whitespace().map(|w| w.to_lowercase()).collect(),
                mode_and: mode != "or",
            },
            None => FilterNode::False,
        },

        // Computed expression comparison.
        Filter::Computed {
            expr_op,
            field,
            cmp,
            value,
            expr_args,
        } => {
            let op = match cmp.as_str() {
                "eq" => CompareOp::Eq,
                "ne" => CompareOp::Ne,
                "gt" => CompareOp::Gt,
                "gte" => CompareOp::Gte,
                "lt" => CompareOp::Lt,
                "lte" => CompareOp::Lte,
                _ => return FilterNode::False,
            };
            match build_index_expr(expr_op, field, expr_args.as_deref(), interner) {
                Some(expr) => FilterNode::ComputedCompare {
                    expr: Box::new(expr),
                    pre_resolved: filter_value_to_query(value),
                    value: value.clone(),
                    op,
                },
                None => FilterNode::False,
            }
        }
    }
}

pub(super) fn build_index_expr(
    expr_op: &str,
    field: &[String],
    _expr_args: Option<&[FilterValue]>,
    interner: &Interner,
) -> Option<crate::index2::expr::IndexExpr> {
    use crate::index2::expr::IndexExpr;
    let path = intern_field_path(field, interner)?;
    let base = IndexExpr::Field(path);
    Some(match expr_op {
        "lower" => IndexExpr::Lower(Box::new(base)),
        "upper" => IndexExpr::Upper(Box::new(base)),
        "trim" => IndexExpr::Trim(Box::new(base)),
        "length" => IndexExpr::Length(Box::new(base)),
        "field" => base,
        // Unknown ops are treated as user-registered scalar names. The
        // brute-force eval path resolves the name via the ScalarResolver
        // at match time; the index path validates `.trusted_pure()` at
        // creation time. If the name doesn't resolve, the eval returns
        // an error → the comparison yields false (no match).
        user_scalar => IndexExpr::Scalar {
            name: user_scalar.to_string(),
            inner: Box::new(base),
        },
    })
}

/// Build a compiled `$in` / `$nin` node.
///
/// If ALL `values` are literals (Null/Bool/Int/Float/String/Binary) we
/// materialise them once into a `TSet<InnerValue>` and emit `FilterNode::InSet`
/// for O(1) membership checks at eval time.  If any value is a non-literal
/// (FieldRef, QueryRef, Fn, Param, …) we fall back to `FilterNode::In` with
/// the pre-resolved parallel slice — the same behaviour as before this
/// optimisation.
fn compile_in_node(
    path: super::filter_node::CompactPath,
    values: &[FilterValue],
    negate: bool,
) -> FilterNode {
    // Attempt to resolve every value to a literal.
    let resolved: Vec<Option<shamir_types::types::value::QueryValue>> =
        values.iter().map(filter_value_to_query).collect();

    if resolved.iter().all(Option::is_some) {
        // Fast path: all literals → HashSet.
        let set: TSet<shamir_types::types::value::QueryValue> =
            resolved.into_iter().flatten().collect();
        FilterNode::InSet {
            field_path: path,
            values: set,
            negate,
        }
    } else {
        // Fallback: mixed literals + dynamic values → linear scan.
        FilterNode::In {
            field_path: path,
            values: values.to_vec(),
            pre_resolved: resolved,
            ref_column_sets: std::sync::OnceLock::new(),
            negate,
        }
    }
}

/// Build a compiled `$contains_any` node.
///
/// When ALL `values` are literals, materialise them into a `TSet<InnerValue>`
/// and emit `FilterNode::ContainsAnySet` for O(1) per-element membership checks
/// instead of the O(N×M) nested scan in `ContainsAny`.
fn compile_contains_any_node(
    path: super::filter_node::CompactPath,
    values: &[FilterValue],
) -> FilterNode {
    let resolved: Vec<Option<shamir_types::types::value::QueryValue>> =
        values.iter().map(filter_value_to_query).collect();
    if resolved.iter().all(Option::is_some) {
        let set: TSet<shamir_types::types::value::QueryValue> =
            resolved.into_iter().flatten().collect();
        FilterNode::ContainsAnySet {
            field_path: path,
            values: set,
        }
    } else {
        FilterNode::ContainsAny {
            field_path: path,
            values: values.to_vec(),
        }
    }
}

/// Build a compiled `$contains_all` node.
///
/// When ALL `values` are literals, materialise them into a `TSet<InnerValue>`
/// and emit `FilterNode::ContainsAllSet` for O(field_len) counting instead of
/// the O(N×M) nested scan in `ContainsAll`.
fn compile_contains_all_node(
    path: super::filter_node::CompactPath,
    values: &[FilterValue],
) -> FilterNode {
    let resolved: Vec<Option<shamir_types::types::value::QueryValue>> =
        values.iter().map(filter_value_to_query).collect();
    if resolved.iter().all(Option::is_some) {
        let set: TSet<shamir_types::types::value::QueryValue> =
            resolved.into_iter().flatten().collect();
        FilterNode::ContainsAllSet {
            field_path: path,
            values: set,
        }
    } else {
        FilterNode::ContainsAll {
            field_path: path,
            values: values.to_vec(),
        }
    }
}

pub(super) fn compile_compare(
    field: &[String],
    value: &FilterValue,
    op: CompareOp,
    interner: &Interner,
) -> FilterNode {
    match intern_field_path_compact(field, interner) {
        Some(path) => FilterNode::Compare {
            field_path: path,
            pre_resolved: filter_value_to_query(value),
            value: value.clone(),
            op,
        },
        None => FilterNode::False,
    }
}
