//! Leaf constructors for [`Filter`].
//!
//! Each function produces a single [`Filter`] variant from a field path and
//! a value. Private helpers `fp` / `fv` / `fvs` convert ergonomic builder
//! arguments into their wire representations and are reused by
//! [`super::combinators`] via `pub(super)`.

use shamir_query_types::filter::{Filter, FilterValue};

use crate::val::IntoFieldPath;

// ── private helpers ───────────────────────────────────────────────────

pub(super) fn fp(field: impl IntoFieldPath) -> Vec<String> {
    field.into_field_path()
}

pub(super) fn fv(value: impl Into<FilterValue>) -> FilterValue {
    value.into()
}

pub(super) fn fvs(values: impl IntoIterator<Item = impl Into<FilterValue>>) -> Vec<FilterValue> {
    values.into_iter().map(Into::into).collect()
}

// ── comparison leaves ────────────────────────────────────────────────

/// `field == value`
pub fn eq(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::Eq {
        field: fp(field),
        value: fv(value),
    }
}

/// `field != value`
pub fn ne(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::Ne {
        field: fp(field),
        value: fv(value),
    }
}

/// `field > value`
pub fn gt(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::Gt {
        field: fp(field),
        value: fv(value),
    }
}

/// `field >= value`
pub fn gte(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::Gte {
        field: fp(field),
        value: fv(value),
    }
}

/// `field < value`
pub fn lt(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::Lt {
        field: fp(field),
        value: fv(value),
    }
}

/// `field <= value`
pub fn lte(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::Lte {
        field: fp(field),
        value: fv(value),
    }
}

// ── field equality shortcut ──────────────────────────────────────────

/// Shortcut equality (the `"field"` op variant on the wire).
pub fn field_eq(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::FieldEq {
        field: fp(field),
        value: fv(value),
    }
}

// ── set membership ───────────────────────────────────────────────────

/// `field IN (values...)`
pub fn in_(
    field: impl IntoFieldPath,
    values: impl IntoIterator<Item = impl Into<FilterValue>>,
) -> Filter {
    Filter::In {
        field: fp(field),
        values: fvs(values),
    }
}

/// `field NOT IN (values...)`
pub fn not_in(
    field: impl IntoFieldPath,
    values: impl IntoIterator<Item = impl Into<FilterValue>>,
) -> Filter {
    Filter::NotIn {
        field: fp(field),
        values: fvs(values),
    }
}

// ── pattern matching ─────────────────────────────────────────────────

/// `field LIKE pattern`
pub fn like(field: impl IntoFieldPath, pattern: impl Into<String>) -> Filter {
    Filter::Like {
        field: fp(field),
        pattern: pattern.into(),
    }
}

/// Case-insensitive `LIKE`.
pub fn ilike(field: impl IntoFieldPath, pattern: impl Into<String>) -> Filter {
    Filter::ILike {
        field: fp(field),
        pattern: pattern.into(),
    }
}

/// `field ~ pattern` (regex match).
pub fn regex(field: impl IntoFieldPath, pattern: impl Into<String>) -> Filter {
    Filter::Regex {
        field: fp(field),
        pattern: pattern.into(),
    }
}

// ── null / existence checks ──────────────────────────────────────────

/// `field IS NULL`
pub fn is_null(field: impl IntoFieldPath) -> Filter {
    Filter::IsNull { field: fp(field) }
}

/// `field IS NOT NULL`
pub fn is_not_null(field: impl IntoFieldPath) -> Filter {
    Filter::IsNotNull { field: fp(field) }
}

/// Field exists in the record.
pub fn exists(field: impl IntoFieldPath) -> Filter {
    Filter::Exists { field: fp(field) }
}

/// Field does not exist in the record.
pub fn not_exists(field: impl IntoFieldPath) -> Filter {
    Filter::NotExists { field: fp(field) }
}

// ── containment ──────────────────────────────────────────────────────

/// Array field contains `value`.
pub fn contains(field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Filter {
    Filter::Contains {
        field: fp(field),
        value: fv(value),
    }
}

/// Array field contains any of `values`.
pub fn contains_any(
    field: impl IntoFieldPath,
    values: impl IntoIterator<Item = impl Into<FilterValue>>,
) -> Filter {
    Filter::ContainsAny {
        field: fp(field),
        values: fvs(values),
    }
}

/// Array field contains all of `values`.
pub fn contains_all(
    field: impl IntoFieldPath,
    values: impl IntoIterator<Item = impl Into<FilterValue>>,
) -> Filter {
    Filter::ContainsAll {
        field: fp(field),
        values: fvs(values),
    }
}

// ── range ────────────────────────────────────────────────────────────

/// `from <= field <= to`
pub fn between(
    field: impl IntoFieldPath,
    from: impl Into<FilterValue>,
    to: impl Into<FilterValue>,
) -> Filter {
    Filter::Between {
        field: fp(field),
        from: fv(from),
        to: fv(to),
    }
}

// ── full-text search ─────────────────────────────────────────────────

/// Full-text search filter.
pub fn fts(field: impl IntoFieldPath, query: impl Into<String>, mode: impl Into<String>) -> Filter {
    Filter::Fts {
        field: fp(field),
        query: query.into(),
        mode: mode.into(),
    }
}

// ── vector similarity ────────────────────────────────────────────────

/// Top-k nearest-neighbor vector similarity search.
pub fn vector_similarity(field: impl IntoFieldPath, query: Vec<f32>, k: u32) -> Filter {
    Filter::VectorSimilarity {
        field: fp(field),
        query,
        k,
    }
}

// ── computed (functional index) ──────────────────────────────────────

/// Comparison on a computed expression (for functional indexes).
///
/// `expr_op`: `"lower"`, `"upper"`, `"trim"`, `"length"`, `"substring"`, `"mod"`, ...
/// `cmp`: `"eq"`, `"lt"`, `"gt"`, `"lte"`, `"gte"`
///
/// `expr_args` is optional and defaults to `None`.
pub fn computed(
    expr_op: impl Into<String>,
    field: impl IntoFieldPath,
    cmp: impl Into<String>,
    value: impl Into<FilterValue>,
) -> Filter {
    Filter::Computed {
        expr_op: expr_op.into(),
        field: fp(field),
        expr_args: None,
        cmp: cmp.into(),
        value: fv(value),
    }
}

/// Like [`computed`] but with explicit `expr_args`.
pub fn computed_with_args(
    expr_op: impl Into<String>,
    field: impl IntoFieldPath,
    expr_args: impl IntoIterator<Item = impl Into<FilterValue>>,
    cmp: impl Into<String>,
    value: impl Into<FilterValue>,
) -> Filter {
    Filter::Computed {
        expr_op: expr_op.into(),
        field: fp(field),
        expr_args: Some(fvs(expr_args)),
        cmp: cmp.into(),
        value: fv(value),
    }
}
