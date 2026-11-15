//! Aggregate-select parameter helpers: literals, execution constants, params.

use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::{FilterContext, FilterValue};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::value::{InnerValue, QueryValue};

/// Whether a [`FilterValue`] is a literal. Dynamic execution constants are
/// validated after binding, while literal type/range errors are caught early.
pub(in crate::query::read::aggregation) fn is_literal(fv: &FilterValue) -> bool {
    matches!(
        fv,
        FilterValue::Null
            | FilterValue::Bool(_)
            | FilterValue::Int(_)
            | FilterValue::Float(_)
            | FilterValue::String(_)
            | FilterValue::Binary(_)
            | FilterValue::Array(_)
    )
}

/// Factory configuration may depend on this execution, but never on a row.
/// Conditionals and function calls are conservatively excluded: they can carry
/// implicit row inputs. AggregateFn.args is not the aggregate's row expression.
pub(in crate::query::read::aggregation) fn is_execution_constant(fv: &FilterValue) -> bool {
    match fv {
        FilterValue::FieldRef { .. } | FilterValue::Cond { .. } | FilterValue::FnCall { .. } => {
            false
        }
        FilterValue::Array(values) => values.iter().all(is_execution_constant),
        FilterValue::Expr { expr } => expr.args.iter().all(is_execution_constant),
        _ => true,
    }
}

/// Resolve with an empty row and a fresh execution-local context. Preflight
/// children so the evaluator's Null fallback cannot hide an unresolved binding.
pub(in crate::query::read::aggregation) fn resolve_aggregate_param(
    fv: &FilterValue,
    ctx: &FilterContext<'_>,
) -> DbResult<QueryValue> {
    match fv {
        FilterValue::Expr { expr } => {
            for arg in &expr.args {
                resolve_aggregate_param(arg, ctx)?;
            }
        }
        FilterValue::Array(values) => {
            for value in values {
                resolve_aggregate_param(value, ctx)?;
            }
        }
        _ => {}
    }
    let fresh = FilterContext::new(ctx.interner, ctx.resolved_refs)
        .with_params(ctx.params)
        .with_scalars(ctx.scalars.clone());
    resolve_filter_query(fv, &InnerValue::Null, &fresh)
        .ok_or_else(|| DbError::Validation("agg_param_unresolved".into()))
}

/// Extract a literal `f64` from a [`FilterValue`] (`Float` or `Int`).
pub(in crate::query::read::aggregation) fn literal_f64(fv: &FilterValue) -> Option<f64> {
    match fv {
        FilterValue::Float(f) => Some(*f),
        FilterValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// Extract a literal `&str` from a [`FilterValue`] (`String`).
pub(in crate::query::read::aggregation) fn literal_string(fv: &FilterValue) -> Option<&str> {
    match fv {
        FilterValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}
