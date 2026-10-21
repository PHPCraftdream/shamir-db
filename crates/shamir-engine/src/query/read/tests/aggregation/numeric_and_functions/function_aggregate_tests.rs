//! funclib aggregate dispatch tests (former `exec_tests.rs`).

use shamir_funclib::scalar_resolver::ScalarResolver;

use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::FilterValue;
use crate::query::read::exec::*;
use crate::query::read::tests::query_results::select_order_page::helpers::{
    intern, make_records, to_bytes_records,
};
use crate::query::read::*;
use shamir_query_builder::select;
use shamir_query_builder::val::lit;
use shamir_storage::error::DbError;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

// ============================================================================
// funclib aggregate dispatch (SelectItem::AggregateFn -> funclib AggRegistry)
// ============================================================================

#[test]
fn aggregate_fn_median_per_group() {
    let interner = Interner::default();
    let records = make_records(&interner);
    // SELECT city, median(age) AS med_age GROUP BY city
    let select = Select {
        items: vec![
            select::field("city"),
            select::agg_fn("median", "age", "med_age"),
        ],
        distinct: false,
    };
    let group_by = GroupBy::new(["city"]);

    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);
    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    );

    // Groups are emitted in alphabetical key order: LA, then NYC.
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["city"], "LA");
    // LA ages [25, 25] -> lower-median = 25.
    assert_eq!(result[0]["med_age"], 25_i64);
    assert_eq!(result[1]["city"], "NYC");
    // NYC ages [30, 35] -> lower-median (even n) = 30.
    assert_eq!(result[1]["med_age"], 30_i64);
}

#[test]
fn aggregate_fn_count_distinct_all_rows() {
    let interner = Interner::default();
    let records = make_records(&interner);
    // SELECT count_distinct(city) AS cities  (no GROUP BY)
    let select = Select {
        items: vec![select::agg_fn("count_distinct", "city", "cities")],
        distinct: false,
    };
    assert!(has_aggregates(&select));

    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);
    // Distinct cities across the four rows: NYC, LA -> 2.
    assert_eq!(result[0]["cities"], 2_i64);
}

#[test]
fn aggregate_fn_unknown_name_is_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::agg_fn("does_not_exist", "age", "x")],
        distinct: false,
    };

    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);
    // An unregistered aggregate yields a null cell, never a panic.
    assert!(result[0]["x"].is_null());
}

// ============================================================================
// Fix 2 (Finding 8) — user-registered scalars in group-SELECT / aggregate-all
// ============================================================================

/// Build a ScalarResolver with a user-registered scalar `my_double`.
fn resolver_with_user_scalar() -> ScalarResolver {
    let layer = shamir_funclib::scalar_resolver::UserScalarLayer::new();
    layer.register(
        "my_double",
        shamir_funclib::registry::FnEntry::pure(
            |args: &[QueryValue]| -> shamir_funclib::registry::ScalarResult {
                match &args[0] {
                    QueryValue::Int(n) => Ok(QueryValue::Int(n * 2)),
                    _ => Err(shamir_funclib::registry::ScalarError::new("type_mismatch")),
                }
            },
            1,
            Some(1),
        ),
    );
    ScalarResolver::new(std::sync::Arc::new(layer))
}

#[test]
fn aggregate_all_user_scalar_resolves() {
    // Site 4: group-SELECT scalar-function projections must resolve
    // user-registered scalars. Before Fix 2, `build_aggregate_object`
    // built a builtins-only FilterContext — user scalars silently fell back
    // to Null. After Fix 2, the resolver is threaded through from `ctx`.
    let interner = Interner::default();
    let records = make_records(&interner);

    let select = Select {
        items: vec![SelectItem::Function {
            name: "my_double".to_string(),
            args: vec![FilterValue::field_ref("age")],
            alias: Some("doubled_age".to_string()),
        }],
        distinct: false,
    };

    let resolver = resolver_with_user_scalar();
    let result = apply_aggregate_all(&to_bytes_records(&records), &select, &interner, resolver);
    assert_eq!(result.len(), 1);
    // The first record's age is 30 (from make_records), so my_double(30) = 60.
    assert_eq!(result[0]["doubled_age"], 60_i64);
}

// ============================================================================
// Fix 1 — wire params for percentile/string_agg (AggregateFn args)
// ============================================================================

/// Build a record with a single `"score"` field (Int).
fn make_score_record(interner: &Interner, score: i64) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(intern(interner, "score")),
        InnerValue::Int(score),
    );
    InnerValue::Map(map)
}

/// Build records with `"score"` field: [10, 20, 30, 40, 50, 60, 70, 80, 90, 100].
fn make_score_records(interner: &Interner) -> Vec<(RecordId, InnerValue)> {
    (10..=100)
        .step_by(10)
        .map(|s| (RecordId::new(), make_score_record(interner, s)))
        .collect()
}

/// Build a record with a single `"name"` field (Str).
pub(in crate::query::read::tests) fn make_name_record(
    interner: &Interner,
    name: &str,
) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(intern(interner, "name")),
        InnerValue::Str(name.into()),
    );
    InnerValue::Map(map)
}

#[test]
fn percentile_with_args_p90() {
    // SELECT percentile(score, args: [0.9]) → 90th percentile, not median.
    let interner = Interner::default();
    let records = make_score_records(&interner);
    let select = Select {
        items: vec![select::agg_fn_with_args(
            "percentile",
            "score",
            "p90",
            [lit(0.9)],
        )],
        distinct: false,
    };
    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    // p=0.9, n=10 → idx = ceil(9.0)-1 = 8 → sorted[8] = 90.
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["p90"], 90_i64);
}

#[test]
fn percentile_default_is_median() {
    // Regression: percentile with NO args still defaults to p=0.5.
    let interner = Interner::default();
    let records = make_score_records(&interner);
    let select = Select {
        items: vec![select::agg_fn("percentile", "score", "p50")],
        distinct: false,
    };
    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    // p=0.5, n=10 → idx = ceil(5.0)-1 = 4 → sorted[4] = 50.
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["p50"], 50_i64);
}

#[test]
fn string_agg_with_custom_sep() {
    // SELECT string_agg(name, args: ["; "]) → joins with "; ", not ",".
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::agg_fn_with_args(
            "string_agg",
            "name",
            "joined",
            [lit("; ")],
        )],
        distinct: false,
    };
    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["joined"], "Alice; Bob; Carol; Dave");
}

#[test]
fn string_agg_default_sep() {
    // Regression: string_agg with NO args still defaults to sep=",".
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::agg_fn("string_agg", "name", "joined")],
        distinct: false,
    };
    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["joined"], "Alice,Bob,Carol,Dave");
}

#[test]
fn validate_rejects_percentile_p_out_of_range() {
    let select = Select {
        items: vec![select::agg_fn_with_args(
            "percentile",
            "score",
            "p",
            [lit(1.5)],
        )],
        distinct: false,
    };
    let err = validate_aggregate_select(&select).unwrap_err();
    match err {
        DbError::Validation(msg) => assert!(msg.contains("out_of_range"), "got: {msg}"),
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validate_rejects_dynamic_arg_to_percentile() {
    // A $ref (field reference) is NOT a literal → rejected.
    let select = Select {
        items: vec![SelectItem::AggregateFn {
            name: "percentile".into(),
            field: AggregateField::Field(vec!["score".into()]),
            input: None,
            args: vec![FilterValue::field_ref("some_field")],
            alias: Some("p".into()),
            distinct: false,
        }],
        distinct: false,
    };
    let err = validate_aggregate_select(&select).unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(msg.contains("agg_param_must_be_literal"), "got: {msg}")
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validate_rejects_args_on_unsupported_aggregate() {
    // "median" does not accept params → rejected.
    let select = Select {
        items: vec![select::agg_fn_with_args("median", "age", "m", [lit(0.5)])],
        distinct: false,
    };
    let err = validate_aggregate_select(&select).unwrap_err();
    match err {
        DbError::Validation(msg) => assert!(msg.contains("agg_params_not_supported"), "got: {msg}"),
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validate_accepts_select_expression_mixed_into_aggregate_select() {
    // #1024 (follow-up to F-26 / #819): a computed `SelectItem::Expression`
    // mixed into an aggregate SELECT (alongside a legitimate CountAll) is no
    // longer rejected — `build_aggregate_object` now evaluates it via the
    // same `func_slots`/`resolve_filter_query` pipeline `SelectItem::
    // Function` uses. Proves the OLD `select_expression_not_supported`
    // rejection is genuinely gone from the aggregate-validation path, not
    // just the plain-projection path.
    let select = Select {
        items: vec![
            select::count_all("cnt"),
            SelectItem::Expression {
                expr: shamir_query_types::read::SelectExpr::Literal {
                    value: shamir_query_types::read::SelectExprValue::Int(1),
                },
                alias: Some("computed".into()),
            },
        ],
        distinct: false,
    };
    assert!(
        validate_aggregate_select(&select).is_ok(),
        "SelectItem::Expression must no longer be rejected in an aggregate SELECT"
    );
}
