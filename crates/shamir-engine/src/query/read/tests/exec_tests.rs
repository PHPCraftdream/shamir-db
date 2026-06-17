//! Tests for the read query execution pipeline (exec.rs).

use serde_json::json;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::exec::*;
use crate::query::read::*;
use shamir_query_builder::select;
use shamir_query_builder::val::{col, func as vfunc, lit};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

/// Convert a slice of QueryValue to Vec<json::Value> for assertion convenience.
fn to_json(qvs: &[QueryValue]) -> Vec<serde_json::Value> {
    qvs.iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect()
}

/// Helper: intern a string and return its u64 id.
fn intern(interner: &Interner, s: &str) -> u64 {
    match interner.touch_ind(s) {
        Ok(TouchInd::New(k)) | Ok(TouchInd::Exists(k)) => k.id(),
        Err(e) => panic!("intern failed: {}", e),
    }
}

/// Build a simple record: `{ "name": Str, "age": Int, "city": Str }`.
fn make_record(interner: &Interner, name: &str, age: i64, city: &str) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(intern(interner, "name")),
        InnerValue::Str(name.into()),
    );
    map.insert(
        InternerKey::new(intern(interner, "age")),
        InnerValue::Int(age),
    );
    map.insert(
        InternerKey::new(intern(interner, "city")),
        InnerValue::Str(city.into()),
    );
    InnerValue::Map(map)
}

fn make_records(interner: &Interner) -> Vec<(RecordId, InnerValue)> {
    vec![
        (RecordId::new(), make_record(interner, "Alice", 30, "NYC")),
        (RecordId::new(), make_record(interner, "Bob", 25, "LA")),
        (RecordId::new(), make_record(interner, "Carol", 35, "NYC")),
        (RecordId::new(), make_record(interner, "Dave", 25, "LA")),
    ]
}

// ============================================================================
// apply_select tests
// ============================================================================

#[test]
fn select_all() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select::all();

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result.len(), 4);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["age"], 30);
}

#[test]
fn select_specific_fields() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select::fields(["name", "age"]);

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result.len(), 4);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["age"], 30);
    assert!(result[0].get("city").is_none());
}

#[test]
fn select_with_alias() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::field_as("name", "user_name")],
        distinct: false,
    };

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["user_name"], "Alice");
    assert!(result[0].get("name").is_none());
}

#[test]
fn select_nonexistent_field_returns_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::field("name"), select::field("nonexistent")],
        distinct: false,
    };

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["name"], "Alice");
    assert!(result[0]["nonexistent"].is_null());
}

// ============================================================================
// scalar function projection (SelectItem::Function)
// ============================================================================

#[test]
fn select_scalar_function_projection() {
    let interner = Interner::default();
    let records = make_records(&interner);
    // SELECT name, strings/upper(name) AS upper_name
    let select = Select {
        items: vec![
            select::field("name"),
            select::func("upper_name", "strings/upper", [col("name")]),
        ],
        distinct: false,
    };

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["upper_name"], "ALICE");
    assert_eq!(result[1]["upper_name"], "BOB");
}

#[test]
fn select_scalar_function_nested_and_literal() {
    let interner = Interner::default();
    let records = make_records(&interner);
    // SELECT strings/concat(strings/upper(city), "!") AS shout  -> "NYC!" etc.
    let select = Select {
        items: vec![select::func(
            "shout",
            "strings/concat",
            [vfunc("strings/upper", [col("city")]), lit("!")],
        )],
        distinct: false,
    };

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["shout"], "NYC!");
}

#[test]
fn select_scalar_function_unknown_is_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::func("x", "strings/nope", [col("name")])],
        distinct: false,
    };

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["x"], serde_json::Value::Null);
}

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

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    let r = to_json(&result);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["city"], "LA");
    assert_eq!(r[0]["cnt"], 2);
    assert_eq!(r[1]["city"], "NYC");
    assert_eq!(r[1]["cnt"], 2);
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

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    let r = to_json(&result);
    assert_eq!(r[0]["city"], "LA");
    assert_eq!(r[0]["total_age"], 50);
    assert_eq!(r[0]["avg_age"], 25.0);
    assert_eq!(r[1]["city"], "NYC");
    assert_eq!(r[1]["total_age"], 65);
    assert_eq!(r[1]["avg_age"], 32.5);
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

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    let r = to_json(&result);
    assert_eq!(r[0]["min_age"], 25);
    assert_eq!(r[0]["max_age"], 25);
    assert_eq!(r[1]["min_age"], 30);
    assert_eq!(r[1]["max_age"], 35);
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

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    let r = to_json(&result);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["city"], "NYC");
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

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
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

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
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

    let result = apply_aggregate_all(&records, &select, &interner);
    let r = to_json(&result);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["total"], 4);
    assert_eq!(r[0]["sum_age"], 115);
}

// ============================================================================
// apply_order_by tests
// ============================================================================

#[test]
fn order_by_asc() {
    let mut records = vec![
        json!({"name": "Carol", "age": 35}),
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Bob", "age": 25}),
    ];

    let order = OrderBy::asc("age");
    apply_order_by(&mut records, &order);
    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["age"], 30);
    assert_eq!(records[2]["age"], 35);
}

#[test]
fn order_by_desc() {
    let mut records = vec![
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Bob", "age": 25}),
        json!({"name": "Carol", "age": 35}),
    ];

    let order = OrderBy::desc("age");
    apply_order_by(&mut records, &order);
    assert_eq!(records[0]["age"], 35);
    assert_eq!(records[1]["age"], 30);
    assert_eq!(records[2]["age"], 25);
}

#[test]
fn order_by_multiple_fields() {
    let mut records = vec![
        json!({"city": "NYC", "age": 35}),
        json!({"city": "LA", "age": 30}),
        json!({"city": "LA", "age": 25}),
        json!({"city": "NYC", "age": 30}),
    ];

    let order = OrderBy::new([OrderByItem::asc("city"), OrderByItem::asc("age")]);
    apply_order_by(&mut records, &order);
    assert_eq!(records[0]["city"], "LA");
    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["city"], "LA");
    assert_eq!(records[1]["age"], 30);
    assert_eq!(records[2]["city"], "NYC");
    assert_eq!(records[2]["age"], 30);
    assert_eq!(records[3]["city"], "NYC");
    assert_eq!(records[3]["age"], 35);
}

#[test]
fn order_by_nulls_first() {
    let mut records = vec![
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Bob"}),
        json!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::new([OrderByItem::asc("age").nulls_first()]);
    apply_order_by(&mut records, &order);
    assert!(records[0].get("age").is_none() || records[0]["age"].is_null());
    assert_eq!(records[1]["age"], 25);
    assert_eq!(records[2]["age"], 30);
}

#[test]
fn order_by_nulls_last() {
    let mut records = vec![
        json!({"name": "Bob"}),
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::new([OrderByItem::asc("age").nulls_last()]);
    apply_order_by(&mut records, &order);
    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["age"], 30);
    assert!(records[2].get("age").is_none() || records[2]["age"].is_null());
}

#[test]
fn order_by_empty_records() {
    let mut records: Vec<serde_json::Value> = vec![];
    let order = OrderBy::asc("age");
    apply_order_by(&mut records, &order);
    assert!(records.is_empty());
}

#[test]
fn order_by_single_record() {
    let mut records = vec![json!({"name": "Alice", "age": 30})];
    let order = OrderBy::desc("age");
    apply_order_by(&mut records, &order);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["age"], 30);
}

#[test]
fn order_by_explicit_null_value() {
    let mut records = vec![
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Bob", "age": null}),
        json!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::asc("age");
    apply_order_by(&mut records, &order);
    // Default ASC -> NullsOrder::Last
    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["age"], 30);
    assert!(records[2]["age"].is_null());
}

#[test]
fn order_by_mixed_types() {
    let mut records = vec![
        json!({"val": "hello"}),
        json!({"val": 42}),
        json!({"val": true}),
    ];

    let order = OrderBy::asc("val");
    apply_order_by(&mut records, &order);
    // Mixed types compare as Equal in the default comparator,
    // so original order is preserved (stable sort).
    assert_eq!(records[0]["val"], "hello");
    assert_eq!(records[1]["val"], 42);
    assert_eq!(records[2]["val"], true);
}

#[test]
fn order_by_empty_items() {
    let mut records = vec![
        json!({"name": "Carol"}),
        json!({"name": "Alice"}),
        json!({"name": "Bob"}),
    ];
    let order = OrderBy::new([]);
    apply_order_by(&mut records, &order);
    // Empty order_by -> no sort -> original order preserved
    assert_eq!(records[0]["name"], "Carol");
    assert_eq!(records[1]["name"], "Alice");
    assert_eq!(records[2]["name"], "Bob");
}

// ============================================================================
// apply_pagination tests
// ============================================================================

#[test]
fn pagination_limit_offset() {
    let records = vec![json!(1), json!(2), json!(3), json!(4), json!(5)];

    let pagination = Pagination::LimitOffset {
        limit: Some(2),
        offset: 1,
    };
    let (result, info) = apply_pagination(records, &pagination, true);

    assert_eq!(result, vec![json!(2), json!(3)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert!(info.has_next);
    assert!(info.has_prev);
}

#[test]
fn pagination_page_based() {
    let records = vec![json!(1), json!(2), json!(3), json!(4), json!(5)];

    let pagination = Pagination::page(2, 2);
    let (result, info) = apply_pagination(records, &pagination, true);

    assert_eq!(result, vec![json!(3), json!(4)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert_eq!(info.current_page, Some(2));
    assert!(info.has_next);
    assert!(info.has_prev);
}

#[test]
fn pagination_count_total_false() {
    let records = vec![json!(1), json!(2), json!(3)];

    let pagination = Pagination::LimitOffset {
        limit: Some(2),
        offset: 0,
    };
    let (result, info) = apply_pagination(records, &pagination, false);

    assert_eq!(result, vec![json!(1), json!(2)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, None);
}

#[test]
fn pagination_none_no_count() {
    let records = vec![json!(1), json!(2)];
    let pagination = Pagination::None;
    let (result, info) = apply_pagination(records, &pagination, false);
    assert_eq!(result, vec![json!(1), json!(2)]);
    assert!(info.is_none());
}

// ============================================================================
// apply_distinct tests
// ============================================================================

#[test]
fn distinct_removes_duplicates() {
    let records = vec![
        json!({"a": 1}),
        json!({"a": 2}),
        json!({"a": 1}),
        json!({"a": 3}),
        json!({"a": 2}),
    ];

    let result = apply_distinct(records);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], json!({"a": 1}));
    assert_eq!(result[1], json!({"a": 2}));
    assert_eq!(result[2], json!({"a": 3}));
}

// ============================================================================
// has_aggregates tests
// ============================================================================

#[test]
fn has_aggregates_true() {
    let select = Select {
        items: vec![select::field("name"), select::count_all("count")],
        distinct: false,
    };
    assert!(has_aggregates(&select));
}

#[test]
fn has_aggregates_false() {
    let select = Select::fields(["name", "age"]);
    assert!(!has_aggregates(&select));
}

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
    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    let r = to_json(&result);

    // Groups are emitted in alphabetical key order: LA, then NYC.
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["city"], "LA");
    // LA ages [25, 25] -> lower-median = 25.
    assert_eq!(r[0]["med_age"], 25);
    assert_eq!(r[1]["city"], "NYC");
    // NYC ages [30, 35] -> lower-median (even n) = 30.
    assert_eq!(r[1]["med_age"], 30);
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

    let result = apply_aggregate_all(&records, &select, &interner);
    let r = to_json(&result);
    assert_eq!(r.len(), 1);
    // Distinct cities across the four rows: NYC, LA -> 2.
    assert_eq!(r[0]["cities"], 2);
}

#[test]
fn aggregate_fn_unknown_name_is_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::agg_fn("does_not_exist", "age", "x")],
        distinct: false,
    };

    let result = apply_aggregate_all(&records, &select, &interner);
    let r = to_json(&result);
    assert_eq!(r.len(), 1);
    // An unregistered aggregate yields a null cell, never a panic.
    assert_eq!(r[0]["x"], serde_json::Value::Null);
}
