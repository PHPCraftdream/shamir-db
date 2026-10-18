//! Ergonomic constructors for [`SelectItem`].

use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::{SelectExpr, SelectItem};

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

// ── computed expression (SelectItem::Expression) ────────────────────

/// Computed SELECT expression (`SelectItem::Expression`) with an alias.
///
/// `expr` is a [`SelectExpr`] tree (`Add`/`Sub`/`Mul`/`Div` over field refs
/// and literals — see `shamir_query_types::read::select_expr`); build one
/// directly via `SelectExpr`'s variants (no dedicated builder DSL for
/// `SelectExpr` itself yet, mirroring how `func`'s `args` are built directly
/// via `FilterValue`).
///
/// #1024: evaluated at execution time by translating `expr` into the
/// equivalent `FilterValue::Expr` shape and routing it through the same
/// pipeline `SelectItem::Function` uses.
pub fn expr_as(alias: impl Into<String>, expr: SelectExpr) -> SelectItem {
    SelectItem::Expression {
        expr,
        alias: Some(alias.into()),
    }
}

/// Computed SELECT expression (`SelectItem::Expression`) with no alias —
/// the output key defaults to `"expr"` (see `SelectProjection::new`).
pub fn expr(expr: SelectExpr) -> SelectItem {
    SelectItem::Expression { expr, alias: None }
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
        input: None,
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
        input: None,
        alias: Some(alias.into()),
        distinct: true,
    }
}

/// Generic built-in aggregate over a per-row expression.
/// `field: All` is a placeholder; `input` takes precedence at execution time.
pub fn agg_of(func: AggFunc, input: FilterValue, alias: impl Into<String>) -> SelectItem {
    SelectItem::Aggregate {
        func,
        field: AggregateField::All,
        input: Some(input),
        alias: Some(alias.into()),
        distinct: false,
    }
}

/// Generic built-in aggregate over expression results with a DISTINCT flag.
/// SUM/AVG/COUNT DISTINCT are rejected by fast-path validation; MIN/MAX
/// DISTINCT are accepted because deduplication cannot change extrema.
pub fn agg_distinct_of(func: AggFunc, input: FilterValue, alias: impl Into<String>) -> SelectItem {
    let mut item = agg_of(func, input, alias);
    if let SelectItem::Aggregate { distinct, .. } = &mut item {
        *distinct = true;
    }
    item
}

/// `SUM(input)` over a per-row expression.
pub fn sum_of(input: FilterValue, alias: impl Into<String>) -> SelectItem {
    agg_of(AggFunc::Sum, input, alias)
}

/// `AVG(input)` over a per-row expression.
pub fn avg_of(input: FilterValue, alias: impl Into<String>) -> SelectItem {
    agg_of(AggFunc::Avg, input, alias)
}

/// `MIN(input)` over a per-row expression.
pub fn min_of(input: FilterValue, alias: impl Into<String>) -> SelectItem {
    agg_of(AggFunc::Min, input, alias)
}

/// `MAX(input)` over a per-row expression.
pub fn max_of(input: FilterValue, alias: impl Into<String>) -> SelectItem {
    agg_of(AggFunc::Max, input, alias)
}

/// `COUNT(input)` over a per-row expression (not `COUNT(*)`).
pub fn count_of(input: FilterValue, alias: impl Into<String>) -> SelectItem {
    agg_of(AggFunc::Count, input, alias)
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

/// Funclib aggregate over a per-row expression, without factory arguments.
pub fn agg_fn_of(
    name: impl Into<String>,
    input: FilterValue,
    alias: impl Into<String>,
) -> SelectItem {
    agg_fn_with_args_of(name, input, alias, [])
}

/// Funclib aggregate over distinct expression results.
pub fn agg_fn_distinct_of(
    name: impl Into<String>,
    input: FilterValue,
    alias: impl Into<String>,
) -> SelectItem {
    let mut item = agg_fn_of(name, input, alias);
    if let SelectItem::AggregateFn { distinct, .. } = &mut item {
        *distinct = true;
    }
    item
}

/// Funclib aggregate with separate per-row input and factory configuration.
pub fn agg_fn_with_args_of(
    name: impl Into<String>,
    input: FilterValue,
    alias: impl Into<String>,
    args: impl IntoIterator<Item = FilterValue>,
) -> SelectItem {
    SelectItem::AggregateFn {
        name: name.into(),
        field: AggregateField::All,
        input: Some(input),
        args: args.into_iter().collect(),
        alias: Some(alias.into()),
        distinct: false,
    }
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
        input: None,
        args: args.into_iter().collect(),
        alias: Some(alias.into()),
        distinct: false,
    }
}
