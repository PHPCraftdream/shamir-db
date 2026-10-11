//! Strict aggregate expression inputs and loud legacy wrappers.

use bytes::Bytes;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_builder::{filter, select, val, Query};
use shamir_query_types::read::{GroupBy, Select, SelectItem};
use shamir_storage::error::DbError;
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::query::filter::FilterContext;
use crate::query::read::aggregate::{
    apply_aggregate_all, apply_aggregate_all_with_context, apply_group_by, try_apply_group_by,
};

fn selection(items: impl IntoIterator<Item = SelectItem>) -> Select {
    Query::from("rows").select(items).build().select
}

fn rows(interner: &Interner, values: &[QueryValue]) -> Vec<(RecordId, Bytes)> {
    values
        .iter()
        .map(|value| {
            let mut record = new_map_wc(1);
            record.insert("v".to_string(), value.clone());
            let inner = query_value_to_inner(&QueryValue::Map(record), interner).unwrap();
            (RecordId::new(), inner.to_bytes().unwrap())
        })
        .collect()
}

/// NULL inputs do not count or poison extrema; missing fields are NULL.
/// Before the fix input was ignored: SUM was 0, AVG/MIN/MAX were NULL and
/// COUNT saw whole records, including NULL rows.
#[test]
fn null_expression_results_are_skipped_and_empty_identities_are_precise() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let select = selection([
        select::sum_of(val::col("v"), "sum"),
        select::avg_of(val::col("v"), "avg"),
        select::min_of(val::col("v"), "min"),
        select::max_of(val::col("v"), "max"),
        select::count_of(val::col("v"), "count"),
        select::sum_of(val::mul(val::col("v"), val::lit(2)), "scaled"),
        select::count_of(val::col("missing"), "missing"),
    ]);
    let records = rows(
        &interner,
        &[QueryValue::Null, QueryValue::Int(2), QueryValue::Int(4)],
    );
    let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
    assert_eq!(result.len(), 1);
    for (key, expected) in [
        ("sum", QueryValue::Int(6)),
        ("avg", QueryValue::F64(3.0)),
        ("min", QueryValue::Int(2)),
        ("max", QueryValue::Int(4)),
        ("count", QueryValue::Int(2)),
        ("scaled", QueryValue::Int(12)),
        ("missing", QueryValue::Int(0)),
    ] {
        assert_eq!(result[0].get(key), Some(&expected));
    }
    for records in [Vec::new(), rows(&interner, &[QueryValue::Null])] {
        let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
        for key in ["sum", "count", "scaled", "missing"] {
            assert_eq!(result[0].get(key), Some(&QueryValue::Int(0)));
        }
        for key in ["avg", "min", "max"] {
            assert_eq!(result[0].get(key), Some(&QueryValue::Null));
        }
    }
}

/// Both fallible paths reject nonnumeric inputs, failed expressions, missing
/// execution bindings and scalar-function errors instead of skipping them.
#[test]
fn expression_errors_are_typed_grouped_and_ungrouped() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let records = rows(&interner, &[QueryValue::Int(3)]);
    let group_by = GroupBy::new(["v"]);
    for (input, code) in [
        (val::lit("bad"), "type_mismatch"),
        (val::mul(val::col("v"), val::lit("bad")), "type_mismatch"),
        (
            val::div(val::col("v"), val::lit(0)),
            "agg_input_expression_error",
        ),
        (val::param("missing"), "agg_input_unresolved"),
        (val::qref("missing", "[0].m"), "agg_input_unresolved"),
    ] {
        let select = selection([select::sum_of(input, "sum")]);
        for error in [
            apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap_err(),
            try_apply_group_by(&records, &group_by, &select, &interner, &ctx).unwrap_err(),
        ] {
            assert!(
                matches!(&error, DbError::Validation(actual) if actual == code),
                "{error:?}",
            );
        }
    }
    let select = selection([select::sum_of(val::func("missing/function", []), "sum")]);
    let error = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap_err();
    assert!(matches!(&error, DbError::Function(_)), "{error:?}");
}

/// Mixed incomparable extrema must not silently retain the first row.
#[test]
fn expression_extrema_reject_incomparable_types() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let records = rows(
        &interner,
        &[QueryValue::Int(3), QueryValue::Str("bad".into())],
    );
    for item in [
        select::min_of(val::col("v"), "value"),
        select::max_of(val::col("v"), "value"),
    ] {
        let error = apply_aggregate_all_with_context(&records, &selection([item]), &interner, &ctx)
            .unwrap_err();
        assert!(
            matches!(&error, DbError::Validation(code) if code == "type_mismatch"),
            "{error:?}",
        );
    }
}

/// bool_or rejects integers at accumulate(), not at input evaluation. Before
/// the fix this error was swallowed and an identity result was returned.
#[test]
fn funclib_expression_accumulate_errors_are_not_swallowed() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let records = rows(&interner, &[QueryValue::Int(3)]);
    let select = selection([select::agg_fn_of("bool_or", val::col("v"), "value")]);
    let error = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap_err();
    assert!(
        matches!(&error, DbError::Function(message) if message.contains("type_mismatch")),
        "{error:?}",
    );
}

/// Selected branches are strict; unselected missing bindings are not evaluated.
#[test]
fn conditional_scalar_inputs_evaluate_only_the_selected_branch() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let records = rows(&interner, &[QueryValue::Int(-3), QueryValue::Int(2)]);
    let select = selection([select::sum_of(
        val::cond(
            filter::eq("v", -3),
            val::func("abs", [val::col("v")]),
            val::cond(filter::eq("v", 2), val::col("v"), val::param("missing")),
        ),
        "sum",
    )]);
    let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
    assert_eq!(result[0].get("sum"), Some(&QueryValue::Int(5)));
}

/// Expression accumulation retains the existing exact integer/Big/Dec lanes
/// and promotes to f64 only when a genuinely fractional value enters SUM.
#[test]
fn expression_sum_preserves_numeric_accumulator_lanes() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let select = selection([select::sum_of(val::col("v"), "sum")]);
    for (values, expected) in [
        (
            vec![QueryValue::Int(i64::MAX), QueryValue::Int(1)],
            QueryValue::Big(BigInt::from(i64::MAX) + 1),
        ),
        (
            vec![
                QueryValue::Big(BigInt::from(i64::MAX)),
                QueryValue::Dec(Decimal::new(2, 0)),
                QueryValue::Int(1),
            ],
            QueryValue::Big(BigInt::from(i64::MAX) + 3),
        ),
        (
            vec![QueryValue::Int(2), QueryValue::Dec(Decimal::new(15, 1))],
            QueryValue::F64(3.5),
        ),
    ] {
        let records = rows(&interner, &values);
        let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
        assert_eq!(result[0].get("sum"), Some(&expected));
    }
}

#[test]
#[should_panic(expected = "apply_aggregate_all: aggregate execution failed")]
fn aggregate_all_wrapper_panics_with_clear_message() {
    let interner = Interner::default();
    let select = selection([select::agg_fn_with_args(
        "percentile",
        "v",
        "p",
        [val::lit(2.0)],
    )]);
    apply_aggregate_all(&[], &select, &interner, ScalarResolver::builtins_only());
}

#[test]
#[should_panic(expected = "apply_group_by: aggregate execution failed")]
fn group_by_wrapper_panics_with_clear_message() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let select = selection([select::agg_fn_with_args(
        "percentile",
        "v",
        "p",
        [val::lit(2.0)],
    )]);
    apply_group_by(&[], &GroupBy::new(["v"]), &select, &interner, &ctx);
}
