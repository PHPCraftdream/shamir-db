//! Strict aggregate expression inputs and loud legacy wrappers.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use bytes::Bytes;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_funclib::registry::FnEntry;
use shamir_funclib::scalar_resolver::{ScalarResolver, UserScalarLayer};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn aggregate_argon_input_keeps_worker_live() {
    let ticker = Arc::new(AtomicU64::new(0));
    let ticks = Arc::clone(&ticker);
    let ticker_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1)).await;
            ticks.fetch_add(1, Ordering::Relaxed);
        }
    });
    let result = tokio::spawn(async move {
        let layer = Arc::new(UserScalarLayer::new());
        layer.register(
            "crypto/argon2id",
            FnEntry::pure(
                move |_| {
                    crate::assert_ticker_progress(&ticker);
                    Ok(QueryValue::Int(1))
                },
                0,
                Some(0),
            ),
        );
        let interner = Interner::default();
        let refs = new_map_wc(0);
        let ctx = FilterContext::new(&interner, &refs).with_scalars(ScalarResolver::new(layer));
        let records = rows(&interner, &[QueryValue::Int(3)]);
        let select = selection([select::sum_of(val::func("crypto/argon2id", []), "sum")]);
        let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
        assert_eq!(result[0].get("sum"), Some(&QueryValue::Int(1)));
    })
    .await;
    ticker_task.abort();
    let _ = ticker_task.await;
    result.unwrap();
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
            val::func("cast/to_int", [val::func("math/abs", [val::col("v")])]),
            val::cond(filter::eq("v", 2), val::col("v"), val::param("missing")),
        ),
        "sum",
    )]);
    let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
    assert_eq!(result[0].get("sum"), Some(&QueryValue::Int(5)));
}

/// Stored Int/F64 inputs agree with field SUM; typed Big/Dec parameters
/// retain exact lanes and promote only for genuinely fractional values.
/// Big/Dec row fixtures are invalid here: to_bytes encodes them as strings.
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
                QueryValue::Int(i64::MAX),
                QueryValue::Int(2),
                QueryValue::Int(1),
            ],
            QueryValue::Big(BigInt::from(i64::MAX) + 3),
        ),
        (
            vec![QueryValue::Int(2), QueryValue::F64(1.5)],
            QueryValue::F64(3.5),
        ),
    ] {
        let records = rows(&interner, &values);
        let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
        assert_eq!(result[0].get("sum"), Some(&expected));
        let field_select = selection([select::sum("v", "sum")]);
        let field_result =
            apply_aggregate_all_with_context(&records, &field_select, &interner, &ctx).unwrap();
        assert_eq!(field_result[0].get("sum"), Some(&expected));
        assert_eq!(result, field_result);
    }

    // Parameters bypass the lossy storage encoding, so these exercise the
    // actual typed Big/Dec accumulator arms, not numeric-looking strings.
    let records = rows(
        &interner,
        &[QueryValue::Int(0), QueryValue::Int(1), QueryValue::Int(2)],
    );
    for (first, second, third, expected) in [
        (
            QueryValue::Big(BigInt::from(i64::MAX)),
            QueryValue::Dec(Decimal::new(2, 0)),
            QueryValue::Int(1),
            QueryValue::Big(BigInt::from(i64::MAX) + 3),
        ),
        (
            QueryValue::Int(2),
            QueryValue::Dec(Decimal::new(15, 1)),
            QueryValue::Null,
            QueryValue::F64(3.5),
        ),
    ] {
        let mut params = new_map_wc(3);
        params.insert("first".to_string(), first);
        params.insert("second".to_string(), second);
        params.insert("third".to_string(), third);
        let ctx = FilterContext::new(&interner, &refs).with_params(&params);
        let select = selection([select::sum_of(
            val::cond(
                filter::eq("v", 0),
                val::param("first"),
                val::cond(
                    filter::eq("v", 1),
                    val::param("second"),
                    val::param("third"),
                ),
            ),
            "sum",
        )]);
        let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
        assert_eq!(result[0].get("sum"), Some(&expected));
    }
}

#[test]
fn aggregate_all_wrapper_returns_aggregate_error() {
    let interner = Interner::default();
    let select = selection([select::agg_fn_with_args(
        "percentile",
        "v",
        "p",
        [val::lit(2.0)],
    )]);
    let error =
        apply_aggregate_all(&[], &select, &interner, ScalarResolver::builtins_only()).unwrap_err();
    assert!(matches!(error, DbError::Validation(_)));
}

#[test]
fn group_by_wrapper_returns_aggregate_error() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let select = selection([select::agg_fn_with_args(
        "percentile",
        "v",
        "p",
        [val::lit(2.0)],
    )]);
    let error = apply_group_by(&[], &GroupBy::new(["v"]), &select, &interner, &ctx).unwrap_err();
    assert!(matches!(error, DbError::Validation(_)));
}
