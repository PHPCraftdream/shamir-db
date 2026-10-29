//! Pattern-length cap and pattern-validity guard for `Regex`/`Like`/`ILike`
//! filter nodes.
//!
//! Two defects this closes:
//!
//! 1. **Unbounded pattern length.** `compile_filter`'s `Regex`/`Like`/
//!    `ILike` arms (`compile.rs`) hand the pattern straight to
//!    `Regex::new`/`like_pattern_to_regex` with no length cap — a
//!    pathologically long pattern can build an expensive regex program
//!    (compile-time and/or matching-time blowup), a query-reachable DoS
//!    surface.
//! 2. **Invalid pattern silently folds to `False`.** `compile_filter`'s
//!    fallback for an unparseable `Regex`/`Like`/`ILike` pattern is
//!    `FilterNode::False` — combined with a `NOT` wrapper (common in
//!    generated queries), `DELETE ... WHERE NOT (regex-with-a-typo)`
//!    compiles to effectively `True` and deletes every row with **no error
//!    surfaced to the caller**. This module validates every pattern
//!    up front, at batch-validate time, so an invalid pattern is a hard,
//!    coded `BatchError` instead of a silent full-table match.
//!
//! Both checks run over the FULL `Filter`/`FilterValue` tree (mirroring
//! `shamir_query_types::filter::check_filter_depth`'s traversal shape),
//! since a `Like`/`Regex` node can be nested inside a `$cond`'s embedded
//! `condition: Filter`.

use regex::{Regex, RegexBuilder};

use crate::query::filter::{Filter, FilterValue};

/// Build a regex after enforcing the shared pattern-size cap.
pub(crate) fn build_regex(pattern: &str) -> Result<Regex, String> {
    if pattern.len() > MAX_FILTER_PATTERN_LENGTH {
        return Err(format!(
            "regex pattern length {} exceeds the {} byte cap",
            pattern.len(),
            MAX_FILTER_PATTERN_LENGTH
        ));
    }
    RegexBuilder::new(pattern)
        .size_limit(10 * 1024 * 1024)
        .build()
        .map_err(|e| format!("invalid regex pattern: {e}"))
}

/// Build a LIKE/ILIKE regex after enforcing the shared pattern-size cap.
pub(crate) fn build_like_regex(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
    if pattern.len() > MAX_FILTER_PATTERN_LENGTH {
        return Err(format!(
            "LIKE/ILIKE pattern length {} exceeds the {} byte cap",
            pattern.len(),
            MAX_FILTER_PATTERN_LENGTH
        ));
    }
    let mut regex_str = String::with_capacity(pattern.len() + 4);
    if case_insensitive {
        regex_str.push_str("(?i)");
    }
    regex_str.push('^');
    for ch in pattern.chars() {
        match ch {
            '%' => regex_str.push_str(".*"),
            '_' => regex_str.push('.'),
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '|' | '^' | '$' => {
                regex_str.push('\\');
                regex_str.push(ch);
            }
            _ => regex_str.push(ch),
        }
    }
    regex_str.push('$');
    RegexBuilder::new(&regex_str)
        .size_limit(10 * 1024 * 1024)
        .build()
        .map_err(|e| format!("invalid LIKE/ILIKE pattern: {e}"))
}

/// Maximum byte length for a `Regex`/`Like`/`ILike` pattern. Patterns
/// beyond this are rejected before ever reaching a regex builder.
pub const MAX_FILTER_PATTERN_LENGTH: usize = 64 * 1024;

/// One entry of the combined `Filter`/`FilterValue` pattern-walk stack.
enum PatternNode<'a> {
    Filter(&'a Filter),
    Value(&'a FilterValue),
    SelectExpr(&'a shamir_query_types::read::SelectExpr),
}

/// Validate every `Regex`/`Like`/`ILike` pattern reachable from `filter`
/// (including patterns nested inside a `$cond`'s `condition`) is within
/// [`MAX_FILTER_PATTERN_LENGTH`] AND compiles successfully.
///
/// Uses an explicit heap-allocated stack (iterative, no unbounded
/// recursion) — mirrors `check_filter_depth`'s traversal shape so this
/// checker cannot itself stack-overflow on a pathologically deep tree.
pub fn check_filter_patterns(filter: &Filter) -> Result<(), String> {
    let mut stack: Vec<PatternNode<'_>> = vec![PatternNode::Filter(filter)];
    walk_patterns(&mut stack)
}

pub fn check_filter_value_patterns(value: &FilterValue) -> Result<(), String> {
    let mut stack = vec![PatternNode::Value(value)];
    walk_patterns(&mut stack)
}

pub fn check_select_expr_patterns(
    expr: &shamir_query_types::read::SelectExpr,
) -> Result<(), String> {
    let mut stack = vec![PatternNode::SelectExpr(expr)];
    walk_patterns(&mut stack)
}

fn walk_patterns(stack: &mut Vec<PatternNode<'_>>) -> Result<(), String> {
    while let Some(node) = stack.pop() {
        match node {
            PatternNode::Filter(filter) => check_and_push_filter(filter, stack)?,
            PatternNode::Value(value) => push_value_children(value, stack),
            PatternNode::SelectExpr(expr) => push_select_expr_children(expr, stack),
        }
    }
    Ok(())
}

/// Inspect a filter's pattern, if any, and enqueue its nested surfaces.
fn check_and_push_filter<'a>(
    filter: &'a Filter,
    stack: &mut Vec<PatternNode<'a>>,
) -> Result<(), String> {
    match filter {
        Filter::And { filters } | Filter::Or { filters } => {
            for f in filters {
                stack.push(PatternNode::Filter(f));
            }
        }
        Filter::Not { filter } => stack.push(PatternNode::Filter(filter)),
        Filter::Like { pattern, .. } => check_like_pattern(pattern, false)?,
        Filter::ILike { pattern, .. } => check_like_pattern(pattern, true)?,
        Filter::Regex { pattern, .. } => check_regex_pattern(pattern)?,
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::FieldEq { value, .. }
        | Filter::Contains { value, .. } => stack.push(PatternNode::Value(value)),
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => {
            for v in values {
                stack.push(PatternNode::Value(v));
            }
        }
        Filter::Between { from, to, .. } => {
            stack.push(PatternNode::Value(from));
            stack.push(PatternNode::Value(to));
        }
        Filter::ValueCompare { left, right, .. } => {
            stack.push(PatternNode::Value(left));
            stack.push(PatternNode::Value(right));
        }
        Filter::Computed {
            expr_args, value, ..
        } => {
            if let Some(args) = expr_args {
                for a in args {
                    stack.push(PatternNode::Value(a));
                }
            }
            stack.push(PatternNode::Value(value));
        }
        Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. }
        | Filter::Fts { .. }
        | Filter::VectorSimilarity { .. } => {}
    }
    Ok(())
}

/// Push `value`'s immediate `FilterValue`/`Filter` children onto the
/// pattern-walk stack (a `$cond`'s `condition` is a nested `Filter` that may
/// itself carry a `Like`/`Regex` leaf).
fn push_value_children<'a>(value: &'a FilterValue, stack: &mut Vec<PatternNode<'a>>) {
    match value {
        FilterValue::Array(items) => {
            for item in items {
                stack.push(PatternNode::Value(item));
            }
        }
        FilterValue::FnCall { call } => {
            for arg in call.args() {
                stack.push(PatternNode::Value(arg));
            }
        }
        FilterValue::Expr { expr } => {
            for arg in &expr.args {
                stack.push(PatternNode::Value(arg));
            }
        }
        FilterValue::Cond { cond } => {
            stack.push(PatternNode::Filter(cond.condition.as_ref()));
            stack.push(PatternNode::Value(&cond.then));
            stack.push(PatternNode::Value(&cond.or_else));
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
    expr: &'a shamir_query_types::read::SelectExpr,
    stack: &mut Vec<PatternNode<'a>>,
) {
    use shamir_query_types::read::SelectExpr;
    match expr {
        SelectExpr::Add { left, right }
        | SelectExpr::Sub { left, right }
        | SelectExpr::Mul { left, right }
        | SelectExpr::Div { left, right } => {
            stack.push(PatternNode::SelectExpr(left));
            stack.push(PatternNode::SelectExpr(right));
        }
        SelectExpr::Field { .. } | SelectExpr::Literal { .. } => {}
    }
}

/// the SAME conversion `compile_filter` uses (`like_pattern_to_regex`), so
/// "validates" and "will actually compile at execution time" can never
/// drift apart.
fn check_like_pattern(pattern: &str, case_insensitive: bool) -> Result<(), String> {
    build_like_regex(pattern, case_insensitive).map(|_| ())
}

/// Validate a `Regex` pattern: length cap, then `Regex::new` — the SAME
/// call `compile_filter` makes.
fn check_regex_pattern(pattern: &str) -> Result<(), String> {
    build_regex(pattern).map(|_| ())
}
