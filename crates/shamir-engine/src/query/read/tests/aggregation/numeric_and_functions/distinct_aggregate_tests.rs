//! distinct-on-aggregate tests via DistinctWrapper (former
//! `exec_tests.rs`).

use shamir_funclib::scalar_resolver::ScalarResolver;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::exec::*;
use crate::query::read::*;
use shamir_query_builder::select;
use shamir_storage::error::DbError;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use crate::query::read::tests::query_results::select_order_page::helpers::{make_records, to_bytes_records};
use crate::query::read::tests::aggregation::numeric_and_functions::function_aggregate_tests::make_name_record;

// ============================================================================
// Fix 2a — distinct on AggregateFn via DistinctWrapper
// ============================================================================

#[test]
fn aggregate_fn_distinct_string_agg_deduplicates() {
    // string_agg over ["a","a","b"] with distinct:true → "a,b" (not "a,a,b").
    let interner = Interner::default();
    let records: Vec<(RecordId, InnerValue)> = ["a", "a", "b"]
        .into_iter()
        .map(|n| (RecordId::new(), make_name_record(&interner, n)))
        .collect();
    let select = Select {
        items: vec![SelectItem::AggregateFn {
            name: "string_agg".into(),
            field: AggregateField::Field(vec!["name".into()]),
            input: None,
            args: vec![],
            alias: Some("joined".into()),
            distinct: true,
        }],
        distinct: false,
    };
    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["joined"], "a,b");
}

#[test]
fn aggregate_fn_non_distinct_keeps_duplicates() {
    // Regression: same data with distinct:false keeps duplicates.
    let interner = Interner::default();
    let records: Vec<(RecordId, InnerValue)> = ["a", "a", "b"]
        .into_iter()
        .map(|n| (RecordId::new(), make_name_record(&interner, n)))
        .collect();
    let select = Select {
        items: vec![SelectItem::AggregateFn {
            name: "string_agg".into(),
            field: AggregateField::Field(vec!["name".into()]),
            input: None,
            args: vec![],
            alias: Some("joined".into()),
            distinct: false,
        }],
        distinct: false,
    };
    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["joined"], "a,a,b");
}

// ============================================================================
// Fix 2b — distinct on fast-path Aggregate (Sum/Avg/Count rejected, Min/Max OK)
// ============================================================================

#[test]
fn validate_rejects_distinct_sum() {
    let select = Select {
        items: vec![SelectItem::Aggregate {
            func: AggFunc::Sum,
            field: AggregateField::Field(vec!["age".into()]),
            input: None,
            alias: Some("total".into()),
            distinct: true,
        }],
        distinct: false,
    };
    let err = validate_aggregate_select(&select).unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("distinct_not_supported_for_fast_path_agg"),
                "got: {msg}"
            )
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validate_rejects_distinct_avg() {
    let select = Select {
        items: vec![SelectItem::Aggregate {
            func: AggFunc::Avg,
            field: AggregateField::Field(vec!["age".into()]),
            input: None,
            alias: Some("mean".into()),
            distinct: true,
        }],
        distinct: false,
    };
    assert!(validate_aggregate_select(&select).is_err());
}

#[test]
fn validate_rejects_distinct_count() {
    let select = Select {
        items: vec![SelectItem::Aggregate {
            func: AggFunc::Count,
            field: AggregateField::Field(vec!["age".into()]),
            input: None,
            alias: Some("cnt".into()),
            distinct: true,
        }],
        distinct: false,
    };
    assert!(validate_aggregate_select(&select).is_err());
}

#[test]
fn validate_allows_distinct_min_and_max() {
    // Min/Max with distinct:true is allowed (correct no-op).
    for func in [AggFunc::Min, AggFunc::Max] {
        let select = Select {
            items: vec![SelectItem::Aggregate {
                func,
                field: AggregateField::Field(vec!["age".into()]),
                input: None,
                alias: Some("extreme".into()),
                distinct: true,
            }],
            distinct: false,
        };
        validate_aggregate_select(&select)
            .unwrap_or_else(|e| panic!("distinct on {func:?} should be allowed: {e}"));
    }
}

#[test]
fn distinct_min_succeeds_and_produces_correct_result() {
    // Min with distinct:true over duplicate values → same result as distinct:false.
    let interner = Interner::default();
    let records = make_records(&interner); // ages: 30, 25, 35, 25
    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![
            select::field("city"),
            SelectItem::Aggregate {
                func: AggFunc::Min,
                field: AggregateField::Field(vec!["age".into()]),
                input: None,
                alias: Some("min_age".into()),
                distinct: true,
            },
        ],
        distinct: false,
    };
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);
    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    );
    // LA: ages [25, 25] → min 25 (distinct is a no-op for min).
    assert_eq!(result[0]["min_age"], 25_i64);
    // NYC: ages [30, 35] → min 30.
    assert_eq!(result[1]["min_age"], 30_i64);
}

#[test]
fn fast_path_distinct_false_behaves_as_before() {
    // Regression: distinct:false on all five fast-path functions works normally.
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![
            select::sum("age", "s"),
            select::avg("age", "a"),
            select::count("age", "c"),
            select::min("age", "mn"),
            select::max("age", "mx"),
        ],
        distinct: false,
    };
    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["s"], 115_i64); // 30+25+35+25
    assert_eq!(result[0]["c"], 4_i64);
    assert_eq!(result[0]["mn"], 25_i64);
    assert_eq!(result[0]["mx"], 35_i64);
}
