//! Tests for the read query execution pipeline (exec.rs).

use serde_json::json;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::exec::*;
use crate::query::read::*;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

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
    let select: Select = serde_json::from_value(json!({
        "items": [{"type": "all"}]
    }))
    .unwrap();

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result.len(), 4);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["age"], 30);
}

#[test]
fn select_specific_fields() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["name"]},
            {"type": "field", "path": ["age"]}
        ]
    }))
    .unwrap();

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
    let select: Select = serde_json::from_value(json!({
        "items": [{"type": "field", "path": ["name"], "alias": "user_name"}]
    }))
    .unwrap();

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["user_name"], "Alice");
    assert!(result[0].get("name").is_none());
}

#[test]
fn select_nonexistent_field_returns_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["name"]},
            {"type": "field", "path": ["nonexistent"]}
        ]
    }))
    .unwrap();

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["name"], "Alice");
    assert!(result[0]["nonexistent"].is_null());
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

    let group_by: GroupBy = serde_json::from_value(json!({
        "fields": [["city"]]
    }))
    .unwrap();
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["city"]},
            {"type": "count_all", "alias": "cnt"}
        ]
    }))
    .unwrap();

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["city"], "LA");
    assert_eq!(result[0]["cnt"], 2);
    assert_eq!(result[1]["city"], "NYC");
    assert_eq!(result[1]["cnt"], 2);
}

#[test]
fn group_by_sum_avg() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by: GroupBy = serde_json::from_value(json!({"fields": [["city"]]})).unwrap();
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["city"]},
            {"type": "aggregate", "func": "sum", "field": ["age"], "alias": "total_age"},
            {"type": "aggregate", "func": "avg", "field": ["age"], "alias": "avg_age"}
        ]
    }))
    .unwrap();

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result[0]["city"], "LA");
    assert_eq!(result[0]["total_age"], 50);
    assert_eq!(result[0]["avg_age"], 25.0);
    assert_eq!(result[1]["city"], "NYC");
    assert_eq!(result[1]["total_age"], 65);
    assert_eq!(result[1]["avg_age"], 32.5);
}

#[test]
fn group_by_min_max() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by: GroupBy = serde_json::from_value(json!({"fields": [["city"]]})).unwrap();
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["city"]},
            {"type": "aggregate", "func": "min", "field": ["age"], "alias": "min_age"},
            {"type": "aggregate", "func": "max", "field": ["age"], "alias": "max_age"}
        ]
    }))
    .unwrap();

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result[0]["min_age"], 25);
    assert_eq!(result[0]["max_age"], 25);
    assert_eq!(result[1]["min_age"], 30);
    assert_eq!(result[1]["max_age"], 35);
}

#[test]
fn group_by_having() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by: GroupBy = serde_json::from_value(json!({
        "fields": [["city"]],
        "having": {"op": "gt", "field": ["total_age"], "value": 55}
    }))
    .unwrap();
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["city"]},
            {"type": "aggregate", "func": "sum", "field": ["age"], "alias": "total_age"}
        ]
    }))
    .unwrap();

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
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

    let group_by: GroupBy = serde_json::from_value(json!({
        "fields": [["city"], ["age"]]
    }))
    .unwrap();
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["city"]},
            {"type": "field", "path": ["age"]},
            {"type": "count_all", "alias": "cnt"}
        ]
    }))
    .unwrap();

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result.len(), 2);
}

#[test]
fn group_by_empty_input() {
    let interner = Interner::default();
    let records: Vec<(RecordId, InnerValue)> = vec![];
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by: GroupBy = serde_json::from_value(json!({"fields": [["city"]]})).unwrap();
    let select: Select = serde_json::from_value(json!({
        "items": [{"type": "count_all"}]
    }))
    .unwrap();

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

    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "count_all", "alias": "total"},
            {"type": "aggregate", "func": "sum", "field": ["age"], "alias": "sum_age"}
        ]
    }))
    .unwrap();

    let result = apply_aggregate_all(&records, &select, &interner);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["total"], 4);
    assert_eq!(result[0]["sum_age"], 115);
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

    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["age"], "direction": "asc"}]
    }))
    .unwrap();
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

    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["age"], "direction": "desc"}]
    }))
    .unwrap();
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

    let order: OrderBy = serde_json::from_value(json!({
        "items": [
            {"field": ["city"], "direction": "asc"},
            {"field": ["age"], "direction": "asc"}
        ]
    }))
    .unwrap();
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

    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["age"], "direction": "asc", "nulls": "first"}]
    }))
    .unwrap();
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

    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["age"], "direction": "asc", "nulls": "last"}]
    }))
    .unwrap();
    apply_order_by(&mut records, &order);
    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["age"], 30);
    assert!(records[2].get("age").is_none() || records[2]["age"].is_null());
}

#[test]
fn order_by_empty_records() {
    let mut records: Vec<serde_json::Value> = vec![];
    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["age"], "direction": "asc"}]
    }))
    .unwrap();
    apply_order_by(&mut records, &order);
    assert!(records.is_empty());
}

#[test]
fn order_by_single_record() {
    let mut records = vec![json!({"name": "Alice", "age": 30})];
    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["age"], "direction": "desc"}]
    }))
    .unwrap();
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

    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["age"], "direction": "asc"}]
    }))
    .unwrap();
    apply_order_by(&mut records, &order);
    // Default ASC → NullsOrder::Last
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

    let order: OrderBy = serde_json::from_value(json!({
        "items": [{"field": ["val"], "direction": "asc"}]
    }))
    .unwrap();
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
    let order: OrderBy = serde_json::from_value(json!({"items": []})).unwrap();
    apply_order_by(&mut records, &order);
    // Empty order_by → no sort → original order preserved
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

    let pagination: Pagination = serde_json::from_value(json!({
        "mode": "LimitOffset", "limit": 2, "offset": 1
    }))
    .unwrap();
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

    let pagination: Pagination = serde_json::from_value(json!({
        "mode": "Page", "page": 2, "page_size": 2
    }))
    .unwrap();
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

    let pagination: Pagination = serde_json::from_value(json!({
        "mode": "LimitOffset", "limit": 2, "offset": 0
    }))
    .unwrap();
    let (result, info) = apply_pagination(records, &pagination, false);

    assert_eq!(result, vec![json!(1), json!(2)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, None);
}

#[test]
fn pagination_none_no_count() {
    let records = vec![json!(1), json!(2)];
    let pagination: Pagination = serde_json::from_value(json!({"mode": "None"})).unwrap();
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
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["name"]},
            {"type": "count_all"}
        ]
    }))
    .unwrap();
    assert!(has_aggregates(&select));
}

#[test]
fn has_aggregates_false() {
    let select: Select = serde_json::from_value(json!({
        "items": [
            {"type": "field", "path": ["name"]},
            {"type": "field", "path": ["age"]}
        ]
    }))
    .unwrap();
    assert!(!has_aggregates(&select));
}
