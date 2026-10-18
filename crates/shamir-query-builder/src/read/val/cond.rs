//! [`FilterValue::Cond`] constructor — the `$cond` ternary operator.

use shamir_query_types::filter::{Cond, Filter, FilterValue};

/// Create a [`FilterValue::Cond`] ternary: yields `then` when `condition`
/// holds, otherwise `or_else`.
///
/// `condition` uses the existing [`Filter`] syntax (the same shape as a
/// `WHERE` clause).
///
/// ```ignore
/// cond(Filter::Eq { field: vec!["active".into()], value: lit(true) },
///      lit("yes"),
///      lit("no"))
/// // → { "$cond": { "if": { "op": "eq", "field": ["active"], "value": true },
/// //                "then": "yes",
/// //                "else": "no" } }
/// ```
pub fn cond(
    condition: Filter,
    then: impl Into<FilterValue>,
    or_else: impl Into<FilterValue>,
) -> FilterValue {
    FilterValue::Cond {
        cond: Box::new(Cond::new(condition, then.into(), or_else.into())),
    }
}

/// Fold an ordered list of `(condition, value)` cases plus a `default` into a
/// right-associated chain of nested [`cond`] calls — the switch-case
/// ergonomic sugar over hand-nesting `cond(cond(cond(...)))`.
///
/// Cases are evaluated in order: the first `condition` that holds wins; if
/// none hold, `default` is the result. Semantically and structurally
/// equivalent to folding `cases` right-to-left with `cond` and `default` as
/// the base case.
///
/// ```ignore
/// switch_case(
///     vec![
///         (Filter::Gte { field: vec!["score".into()], value: lit(100_i64) }, lit("vip")),
///         (Filter::Gte { field: vec!["score".into()], value: lit(50_i64) }, lit("regular")),
///     ],
///     lit("newbie"),
/// )
/// // == cond(score >= 100, "vip", cond(score >= 50, "regular", "newbie"))
/// ```
pub fn switch_case(
    cases: Vec<(Filter, FilterValue)>,
    default: impl Into<FilterValue>,
) -> FilterValue {
    cases
        .into_iter()
        .rev()
        .fold(default.into(), |acc, (condition, then)| {
            cond(condition, then, acc)
        })
}
