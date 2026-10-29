//! Aggregate-select pre-execution validation.
//!
//! Split out of the former `read/aggregate.rs` (bodies unchanged).

use shamir_storage::error::DbError;

use crate::query::read::{AggFunc, Select, SelectItem};

use super::accumulator::{is_execution_constant, is_literal, literal_f64, literal_string};

/// Validate all aggregate select items **before** execution, converting the
/// silent-wrong-behaviour bugs from the release-readiness report into honest,
/// coded rejections.
///
/// - `AggregateFn` with non-empty `args`: only `percentile` and `string_agg`
///   accept one execution-constant parameter. Row-dependent configuration is
///   rejected with `agg_param_must_be_literal` for legacy compatibility. Literal
///   type/range errors are checked here; dynamic values are checked at execution.
/// - `Aggregate { distinct: true, .. }` with `func` = `Sum`/`Avg`/`Count`:
///   rejected (`distinct_not_supported_for_fast_path_agg`) because the
///   hand-rolled `AggAccum` state machine has no distinct-dedup tracking.
///   `Min`/`Max` with `distinct: true` are ALLOWED (distinct is a correct
///   no-op for min/max).
pub fn validate_aggregate_select(select: &Select) -> shamir_storage::error::DbResult<()> {
    for item in &select.items {
        match item {
            SelectItem::AggregateFn { name, args, .. } if !args.is_empty() => match name.as_str() {
                "percentile" => {
                    if args.len() != 1 {
                        return Err(DbError::Validation("agg_param_arity".into()));
                    }
                    if !is_execution_constant(&args[0]) {
                        return Err(shamir_storage::error::DbError::Validation(
                            "agg_param_must_be_literal".into(),
                        ));
                    }
                    if !is_literal(&args[0]) {
                        continue;
                    }
                    let p = literal_f64(&args[0]).ok_or_else(|| {
                        shamir_storage::error::DbError::Validation("type_mismatch".into())
                    })?;
                    if !(0.0..=1.0).contains(&p) {
                        return Err(shamir_storage::error::DbError::Validation(
                            "out_of_range".into(),
                        ));
                    }
                }
                "string_agg" => {
                    if args.len() != 1 {
                        return Err(DbError::Validation("agg_param_arity".into()));
                    }
                    if !is_execution_constant(&args[0]) {
                        return Err(shamir_storage::error::DbError::Validation(
                            "agg_param_must_be_literal".into(),
                        ));
                    }
                    if !is_literal(&args[0]) {
                        continue;
                    }
                    let _ = literal_string(&args[0]).ok_or_else(|| {
                        shamir_storage::error::DbError::Validation("type_mismatch".into())
                    })?;
                }
                other => {
                    return Err(shamir_storage::error::DbError::Validation(format!(
                        "agg_params_not_supported: {other}"
                    )));
                }
            },
            SelectItem::Aggregate { func, distinct, .. } if *distinct => {
                if matches!(func, AggFunc::Sum | AggFunc::Avg | AggFunc::Count) {
                    return Err(shamir_storage::error::DbError::Validation(format!(
                        "distinct_not_supported_for_fast_path_agg: {func:?}"
                    )));
                }
                // Min/Max: distinct is a correct no-op (min/max of a set ==
                // min/max of its distinct subset). Allow silently.
            }
            // #1024: `SelectItem::Expression` in an aggregate context is no
            // longer rejected — `build_aggregate_object` now evaluates it via
            // the same `func_slots`/`resolve_filter_query` pipeline
            // `SelectItem::Function` uses (translated through
            // `SelectExpr::to_filter_value`), same as the plain-projection
            // path (`SelectProjection::new`). No validation needed here: a
            // computed expression carries no `args`/`distinct` shape to
            // check, unlike `AggregateFn`/`Aggregate` above.
            _ => {}
        }
    }
    Ok(())
}
