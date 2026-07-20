//! Ergonomic constructors for [`SelectItem`].

use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::SelectItem;

use crate::val::IntoFieldPath;

pub use shamir_query_types::read::{AggFunc, AggregateField};

// ── wildcard ────────────────────────────────────────────────────────

/// Select all fields (`SELECT *`).
pub fn all() -> SelectItem {
    SelectItem::All
}

// ── field ───────────────────────────────────────────────────────────

/// Select a single field (no alias).
pub fn field(path: impl IntoFieldPath) -> SelectItem {
    SelectItem::Field {
        path: path.into_field_path(),
        alias: None,
    }
}

/// Select a single field with an alias.
pub fn field_as(path: impl IntoFieldPath, alias: impl Into<String>) -> SelectItem {
    SelectItem::Field {
        path: path.into_field_path(),
        alias: Some(alias.into()),
    }
}

// ── scalar function in projection ───────────────────────────────────

/// Scalar function call in the projection (`SelectItem::Function`).
///
/// `name` is the folder-qualified scalar function name (e.g.
/// `"strings/upper"`). `args` reuse [`FilterValue`] — field refs,
/// literals, and nested `$fn` calls.
pub fn func(
    alias: impl Into<String>,
    name: impl Into<String>,
    args: impl IntoIterator<Item = FilterValue>,
) -> SelectItem {
    SelectItem::Function {
        name: name.into(),
        args: args.into_iter().collect(),
        alias: Some(alias.into()),
    }
}

// ── count_all ───────────────────────────────────────────────────────

/// `COUNT(*)` — counts all records.
pub fn count_all(alias: impl Into<String>) -> SelectItem {
    SelectItem::CountAll {
        alias: Some(alias.into()),
    }
}

// ── built-in aggregates (fast-path: Count/Sum/Avg/Min/Max) ─────────

/// Generic built-in aggregate (non-distinct).
pub fn agg(func: AggFunc, field: impl IntoFieldPath, alias: impl Into<String>) -> SelectItem {
    SelectItem::Aggregate {
        func,
        field: AggregateField::Field(field.into_field_path()),
        alias: Some(alias.into()),
        distinct: false,
    }
}

/// Generic built-in aggregate with `DISTINCT`.
pub fn agg_distinct(
    func: AggFunc,
    field: impl IntoFieldPath,
    alias: impl Into<String>,
) -> SelectItem {
    SelectItem::Aggregate {
        func,
        field: AggregateField::Field(field.into_field_path()),
        alias: Some(alias.into()),
        distinct: true,
    }
}

// ── convenience wrappers ────────────────────────────────────────────

/// `SUM(field)` aggregate.
pub fn sum(field: impl IntoFieldPath, alias: impl Into<String>) -> SelectItem {
    agg(AggFunc::Sum, field, alias)
}

/// `AVG(field)` aggregate.
pub fn avg(field: impl IntoFieldPath, alias: impl Into<String>) -> SelectItem {
    agg(AggFunc::Avg, field, alias)
}

/// `MIN(field)` aggregate.
pub fn min(field: impl IntoFieldPath, alias: impl Into<String>) -> SelectItem {
    agg(AggFunc::Min, field, alias)
}

/// `MAX(field)` aggregate.
pub fn max(field: impl IntoFieldPath, alias: impl Into<String>) -> SelectItem {
    agg(AggFunc::Max, field, alias)
}

/// `COUNT(field)` aggregate (per-field, NOT `COUNT(*)`).
///
/// For `COUNT(*)` use [`count_all`].
pub fn count(field: impl IntoFieldPath, alias: impl Into<String>) -> SelectItem {
    agg(AggFunc::Count, field, alias)
}

// ── funclib aggregates (AggregateFn) ────────────────────────────────

/// Funclib aggregate dispatched by name (e.g. `"median"`, `"stddev"`).
///
/// Non-distinct variant with no static parameters.
pub fn agg_fn(
    name: impl Into<String>,
    field: impl IntoFieldPath,
    alias: impl Into<String>,
) -> SelectItem {
    agg_fn_with_args(name, field, alias, [])
}

/// Funclib aggregate dispatched by name, with `DISTINCT`.
pub fn agg_fn_distinct(
    name: impl Into<String>,
    field: impl IntoFieldPath,
    alias: impl Into<String>,
) -> SelectItem {
    let mut item = agg_fn_with_args(name, field, alias, []);
    if let SelectItem::AggregateFn { distinct, .. } = &mut item {
        *distinct = true;
    }
    item
}

/// Funclib aggregate dispatched by name, with static parameters.
///
/// `args` carries literal parameters for parameterised aggregates
/// (e.g. `0.9` for `percentile`, `";"` for `string_agg`). Dynamic
/// `$ref`/`$fn`/etc. values are rejected at execution time.
pub fn agg_fn_with_args(
    name: impl Into<String>,
    field: impl IntoFieldPath,
    alias: impl Into<String>,
    args: impl IntoIterator<Item = FilterValue>,
) -> SelectItem {
    SelectItem::AggregateFn {
        name: name.into(),
        field: AggregateField::Field(field.into_field_path()),
        args: args.into_iter().collect(),
        alias: Some(alias.into()),
        distinct: false,
    }
}
