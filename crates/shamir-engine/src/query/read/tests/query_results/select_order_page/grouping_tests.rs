//! apply_group_by / aggregate-all execution tests (former `exec_tests.rs`).

use shamir_funclib::scalar_resolver::ScalarResolver;

use super::helpers::{make_record, make_records, to_bytes_records};
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::*;
use shamir_query_builder::select;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ============================================================================
// apply_group_by tests
// ============================================================================

#[test]
fn group_by_count() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![select::field("city"), select::count_all("cnt")],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    )
    .expect("aggregate execution succeeds");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["city"], "LA");
    assert_eq!(result[0]["cnt"], 2_i64);
    assert_eq!(result[1]["city"], "NYC");
    assert_eq!(result[1]["cnt"], 2_i64);
}

#[test]
fn group_by_sum_avg() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![
            select::field("city"),
            select::sum("age", "total_age"),
            select::avg("age", "avg_age"),
        ],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    )
    .expect("aggregate execution succeeds");
    assert_eq!(result[0]["city"], "LA");
    assert_eq!(result[0]["total_age"], 50_i64);
    assert_eq!(result[0]["avg_age"], 25.0_f64);
    assert_eq!(result[1]["city"], "NYC");
    assert_eq!(result[1]["total_age"], 65_i64);
    assert_eq!(result[1]["avg_age"], 32.5_f64);
}

#[test]
fn group_by_min_max() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![
            select::field("city"),
            select::min("age", "min_age"),
            select::max("age", "max_age"),
        ],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    )
    .expect("aggregate execution succeeds");
    assert_eq!(result[0]["min_age"], 25_i64);
    assert_eq!(result[0]["max_age"], 25_i64);
    assert_eq!(result[1]["min_age"], 30_i64);
    assert_eq!(result[1]["max_age"], 35_i64);
}

#[test]
fn group_by_having() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy {
        fields: vec![vec!["city".into()]],
        having: Some(shamir_query_builder::filter::gt("total_age", 55)),
    };
    let select = Select {
        items: vec![select::field("city"), select::sum("age", "total_age")],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    )
    .expect("aggregate execution succeeds");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["city"], "NYC");
}

#[test]
fn group_by_multiple_fields() {
    let interner = Interner::default();
    let records = vec![
        (RecordId::new(), make_record(&interner, "Alice", 25, "NYC")),
        (RecordId::new(), make_record(&interner, "Bob", 25, "NYC")),
        (RecordId::new(), make_record(&interner, "Carol", 30, "NYC")),
    ];
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy {
        fields: vec![vec!["city".into()], vec!["age".into()]],
        having: None,
    };
    let select = Select {
        items: vec![
            select::field("city"),
            select::field("age"),
            select::count_all("cnt"),
        ],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    )
    .expect("aggregate execution succeeds");
    assert_eq!(result.len(), 2);
}

#[test]
fn group_by_empty_input() {
    let interner = Interner::default();
    let records: Vec<(RecordId, InnerValue)> = vec![];
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![SelectItem::CountAll { alias: None }],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    )
    .expect("aggregate execution succeeds");
    assert!(result.is_empty());
}

// ============================================================================
// apply_aggregate_all tests
// ============================================================================

#[test]
fn aggregate_all_count_sum() {
    let interner = Interner::default();
    let records = make_records(&interner);

    let select = Select {
        items: vec![select::count_all("total"), select::sum("age", "sum_age")],
        distinct: false,
    };

    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .expect("aggregate execution succeeds");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["total"], 4_i64);
    assert_eq!(result[0]["sum_age"], 115_i64);
}
