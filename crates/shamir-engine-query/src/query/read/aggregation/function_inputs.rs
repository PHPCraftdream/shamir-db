//! Aggregate-function input handling: the funclib factory configuration
//! boundary and the strict aggregate-input evaluator.
//!
//! Split out of the former `read/aggregate.rs` (bodies unchanged).

use crate::function::builtin_aggs;
use crate::query::filter::{
    compile_filter, compile_filter_with_context, resolve_filter_query, FilterContext, FilterExprOp,
    FilterValue,
};
use shamir_funclib::agg::Aggregator;
use shamir_funclib::registry::ScalarError;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::record_view::RecordRef;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

use super::accumulator::{is_execution_constant, resolve_aggregate_param};

const MAX_STRING_AGG_SEPARATOR_BYTES: usize = 1024;

struct CappedStringAgg {
    separator: String,
    value: String,
    has_value: bool,
    max: usize,
}

impl CappedStringAgg {
    fn new(separator: String, max: usize) -> DbResult<Self> {
        if separator.len() > MAX_STRING_AGG_SEPARATOR_BYTES {
            return Err(DbError::Validation(format!(
                "string_agg separator exceeds {MAX_STRING_AGG_SEPARATOR_BYTES} bytes"
            )));
        }
        Ok(Self {
            separator,
            value: String::new(),
            has_value: false,
            max,
        })
    }
}

impl Aggregator for CappedStringAgg {
    fn accumulate(&mut self, value: &QueryValue) -> Result<(), ScalarError> {
        let text = match value {
            QueryValue::Null => return Ok(()),
            QueryValue::Str(text) => text,
            _ => return Err(ScalarError::new("type_mismatch")),
        };
        let separator_bytes = if self.has_value {
            self.separator.len()
        } else {
            0
        };
        let actual = self
            .value
            .len()
            .saturating_add(separator_bytes)
            .saturating_add(text.len());
        if actual > self.max {
            return Err(ScalarError::new(format!(
                "result_too_large: ResultBytes actual={actual} max={}",
                self.max
            )));
        }
        if actual > self.value.capacity() {
            let capacity = actual
                .max(self.value.capacity().saturating_mul(2))
                .min(self.max);
            self.value.reserve_exact(capacity - self.value.len());
        }
        if self.has_value {
            self.value.push_str(&self.separator);
        }
        self.value.push_str(text);
        self.has_value = true;
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        Ok(QueryValue::Str(self.value))
    }
}

pub(super) fn aggregate_accumulation_error(error: ScalarError) -> DbError {
    if error.code.starts_with("result_too_large: ResultBytes ") {
        DbError::Validation(error.code)
    } else {
        DbError::Function(error.to_string())
    }
}

/// Construct factory configuration from the current execution, not a row.
pub(super) fn make_aggregate_fn(
    name: &str,
    args: &[FilterValue],
    ctx: &FilterContext<'_>,
) -> DbResult<Option<Box<dyn Aggregator>>> {
    if args.is_empty() {
        if name == "string_agg" {
            return Ok(Some(Box::new(CappedStringAgg::new(
                ",".into(),
                ctx.max_accumulator_bytes(),
            )?)));
        }
        return Ok(builtin_aggs().make(name));
    }
    if args.len() != 1 {
        return Err(DbError::Validation("agg_param_arity".into()));
    }
    if !is_execution_constant(&args[0]) {
        return Err(DbError::Validation("agg_param_must_be_literal".into()));
    }
    let value = resolve_aggregate_param(&args[0], ctx)?;
    match name {
        "percentile" => {
            let p = match value {
                QueryValue::F64(p) => p,
                QueryValue::Int(p) => p as f64,
                _ => return Err(DbError::Validation("type_mismatch".into())),
            };
            if !(0.0..=1.0).contains(&p) {
                return Err(DbError::Validation("out_of_range".into()));
            }
            Ok(Some(shamir_funclib::agg::percentile(p)()))
        }
        "string_agg" => {
            let QueryValue::Str(separator) = value else {
                return Err(DbError::Validation("type_mismatch".into()));
            };
            Ok(Some(Box::new(CappedStringAgg::new(
                separator,
                ctx.max_accumulator_bytes(),
            )?)))
        }
        _ => Err(DbError::Validation(format!(
            "agg_params_not_supported: {name}"
        ))),
    }
}

/// Strict aggregate-input boundary around the Option-returning filter evaluator.
/// Missing fields are NULL; unresolved bindings and evaluation failures are errors.
/// Only the selected conditional branch is evaluated. Arithmetic inherits the
/// filter evaluator's numeric promotion and overflow behaviour. Arithmetic
/// NULL operands yield NULL (and short-circuit); other operators retain the
/// filter evaluator's semantics. Conditional predicates retain WHERE's
/// missing-binding/non-match semantics; only branch values are strict.
pub(super) fn resolve_aggregate_input(
    fv: &FilterValue,
    record: &dyn RecordRef,
    ctx: &FilterContext<'_>,
) -> DbResult<QueryValue> {
    match fv {
        FilterValue::Expr { expr } => {
            let mut params = new_map_wc(expr.args.len());
            let mut bound = expr.clone();
            bound.args.clear();
            for (index, arg) in expr.args.iter().enumerate() {
                let value = resolve_aggregate_input(arg, record, ctx)?;
                if matches!(value, QueryValue::Null)
                    && matches!(
                        expr.op,
                        FilterExprOp::Add
                            | FilterExprOp::Sub
                            | FilterExprOp::Mul
                            | FilterExprOp::Div
                            | FilterExprOp::Mod
                            | FilterExprOp::Neg
                    )
                {
                    return Ok(QueryValue::Null);
                }
                if matches!(
                    expr.op,
                    FilterExprOp::Add
                        | FilterExprOp::Sub
                        | FilterExprOp::Mul
                        | FilterExprOp::Div
                        | FilterExprOp::Mod
                        | FilterExprOp::Neg
                ) && !matches!(
                    value,
                    QueryValue::Int(_) | QueryValue::F64(_) | QueryValue::Dec(_)
                ) {
                    return Err(DbError::Validation("type_mismatch".into()));
                }
                let name = index.to_string();
                params.insert(name.clone(), value);
                bound.args.push(FilterValue::Param { name });
            }
            let fresh = FilterContext::new(ctx.interner, ctx.resolved_refs)
                .with_params(&params)
                .with_scalars(ctx.scalars.clone())
                .with_overlay_snapshot(ctx.interner_overlay.clone())
                .with_overlay_ids_snapshot(ctx.interner_overlay_ids.clone());
            resolve_filter_query(&FilterValue::Expr { expr: bound }, record, &fresh)
                .ok_or_else(|| DbError::Validation("agg_input_expression_error".into()))
        }
        FilterValue::FnCall { call } => {
            let args = call
                .args()
                .iter()
                .map(|arg| resolve_aggregate_input(arg, record, ctx))
                .collect::<DbResult<Vec<_>>>()?;
            ctx.scalars
                .call(call.name(), &args)
                .map_err(|error| DbError::Function(error.to_string()))
        }
        FilterValue::Cond { cond } => {
            let matched = if ctx.interner_overlay_ids.is_some() {
                compile_filter_with_context(&cond.condition, ctx).matches(record, ctx)
            } else {
                ctx.cond_cache
                    .and_then(|cache| crate::query::filter::cond_cache_get(cache, &cond.condition))
                    .map_or_else(
                        || compile_filter(&cond.condition, ctx.interner).matches(record, ctx),
                        |node| node.matches(record, ctx),
                    )
            };
            resolve_aggregate_input(
                if matched { &cond.then } else { &cond.or_else },
                record,
                ctx,
            )
        }
        FilterValue::Array(values) => values
            .iter()
            .map(|value| resolve_aggregate_input(value, record, ctx))
            .collect::<DbResult<Vec<_>>>()
            .map(QueryValue::List),
        FilterValue::FieldRef { .. } => {
            Ok(resolve_filter_query(fv, record, ctx).unwrap_or(QueryValue::Null))
        }
        _ => resolve_filter_query(fv, record, ctx)
            .ok_or_else(|| DbError::Validation("agg_input_unresolved".into())),
    }
}
