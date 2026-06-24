//! [`FilterValue::Expr`] constructors — arithmetic / string / logic / comparison
//! expressions built via the `$expr` wire variant.

use shamir_query_types::filter::{FilterExpr, FilterExprOp, FilterValue};

/// Create a [`FilterValue::Expr`] with the given operator and arguments.
///
/// This is the generic escape-hatch; prefer the named wrappers (`add`, `sub`,
/// `concat`, …) below for readability.
///
/// ```ignore
/// expr(FilterExprOp::Add, [col("price"), lit(10)])
/// // → { "$expr": { "op": "add", "args": [{ "$ref": ["price"] }, 10] } }
/// ```
pub fn expr(op: FilterExprOp, args: impl IntoIterator<Item = FilterValue>) -> FilterValue {
    FilterValue::Expr {
        expr: FilterExpr::new(op, args.into_iter().collect()),
    }
}

// ── arithmetic (binary) ──────────────────────────────────────────────

/// `$expr` `add`: `a + b`.
pub fn add(a: impl Into<FilterValue>, b: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Add, [a.into(), b.into()])
}

/// `$expr` `sub`: `a - b`.
pub fn sub(a: impl Into<FilterValue>, b: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Sub, [a.into(), b.into()])
}

/// `$expr` `mul`: `a * b`.
pub fn mul(a: impl Into<FilterValue>, b: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Mul, [a.into(), b.into()])
}

/// `$expr` `div`: `a / b`.
pub fn div(a: impl Into<FilterValue>, b: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Div, [a.into(), b.into()])
}

/// `$expr` `mod`: `a % b`.
pub fn modulo(a: impl Into<FilterValue>, b: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Mod, [a.into(), b.into()])
}

/// `$expr` `neg`: unary negation `-x`.
pub fn neg(x: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Neg, [x.into()])
}

// ── string ───────────────────────────────────────────────────────────

/// `$expr` `concat`: string-concatenate all `parts`.
pub fn concat(parts: impl IntoIterator<Item = FilterValue>) -> FilterValue {
    expr(FilterExprOp::Concat, parts)
}

/// `$expr` `lower`: lowercase a string.
pub fn lower(x: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Lower, [x.into()])
}

/// `$expr` `upper`: uppercase a string.
pub fn upper(x: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Upper, [x.into()])
}

/// `$expr` `trim`: trim whitespace from a string.
pub fn trim(x: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Trim, [x.into()])
}

/// `$expr` `length`: string length.
pub fn length(x: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Length, [x.into()])
}

// ── logic ────────────────────────────────────────────────────────────
//
// Named `*_expr` (not bare `and`/`or`/`not`) to avoid clashing with the
// [`crate::filter`] combinators of the same name, which are glob-imported
// alongside `val::*` in many call sites. Mirrors the precedent set by
// [`shamir_query_builder::filter::FilterExt::negate`].

/// `$expr` `and`: logical AND of all `parts`.
pub fn and_expr(parts: impl IntoIterator<Item = FilterValue>) -> FilterValue {
    expr(FilterExprOp::And, parts)
}

/// `$expr` `or`: logical OR of all `parts`.
pub fn or_expr(parts: impl IntoIterator<Item = FilterValue>) -> FilterValue {
    expr(FilterExprOp::Or, parts)
}

/// `$expr` `not`: logical NOT.
pub fn not_expr(x: impl Into<FilterValue>) -> FilterValue {
    expr(FilterExprOp::Not, [x.into()])
}
