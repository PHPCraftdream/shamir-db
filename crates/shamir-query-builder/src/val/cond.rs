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
