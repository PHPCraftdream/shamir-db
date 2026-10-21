//! apply_order_by_qv / apply_pagination / apply_distinct_qv / has_aggregates
//! execution tests (former `exec_tests.rs`).

use shamir_query_builder::select;
use shamir_types::mpack;

use crate::query::read::exec::*;
use crate::query::read::*;
use shamir_types::types::value::QueryValue;

// ============================================================================
// apply_order_by_qv tests (QueryValue-native path)
// ============================================================================

#[test]
fn order_by_asc() {
    let mut records = vec![
        mpack!({"name": "Carol", "age": 35}),
        mpack!({"name": "Alice", "age": 30}),
        mpack!({"name": "Bob", "age": 25}),
    ];

    let order = OrderBy::asc("age");
    apply_order_by_qv(&mut records, &order);
    assert_eq!(records[0]["age"], 25_i64);
    assert_eq!(records[1]["age"], 30_i64);
    assert_eq!(records[2]["age"], 35_i64);
}

#[test]
fn order_by_desc() {
    let mut records = vec![
        mpack!({"name": "Alice", "age": 30}),
        mpack!({"name": "Bob", "age": 25}),
        mpack!({"name": "Carol", "age": 35}),
    ];

    let order = OrderBy::desc("age");
    apply_order_by_qv(&mut records, &order);
    assert_eq!(records[0]["age"], 35_i64);
    assert_eq!(records[1]["age"], 30_i64);
    assert_eq!(records[2]["age"], 25_i64);
}

#[test]
fn order_by_multiple_fields() {
    let mut records = vec![
        mpack!({"city": "NYC", "age": 35}),
        mpack!({"city": "LA", "age": 30}),
        mpack!({"city": "LA", "age": 25}),
        mpack!({"city": "NYC", "age": 30}),
    ];

    let order = OrderBy::new([OrderByItem::asc("city"), OrderByItem::asc("age")]);
    apply_order_by_qv(&mut records, &order);
    assert_eq!(records[0]["city"], "LA");
    assert_eq!(records[0]["age"], 25_i64);
    assert_eq!(records[1]["city"], "LA");
    assert_eq!(records[1]["age"], 30_i64);
    assert_eq!(records[2]["city"], "NYC");
    assert_eq!(records[2]["age"], 30_i64);
    assert_eq!(records[3]["city"], "NYC");
    assert_eq!(records[3]["age"], 35_i64);
}

#[test]
fn order_by_nulls_first() {
    // Bob has no "age" field -- missing maps to Null, sorted first by nulls_first.
    let mut records = vec![
        mpack!({"name": "Alice", "age": 30}),
        mpack!({"name": "Bob"}),
        mpack!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::new([OrderByItem::asc("age").nulls_first()]);
    apply_order_by_qv(&mut records, &order);
    // Bob (no age) must come first
    assert!(records[0]["age"].is_null());
    assert_eq!(records[1]["age"], 25_i64);
    assert_eq!(records[2]["age"], 30_i64);
}

#[test]
fn order_by_nulls_last() {
    // Bob has no "age" field -- missing maps to Null, sorted last by default ASC.
    let mut records = vec![
        mpack!({"name": "Bob"}),
        mpack!({"name": "Alice", "age": 30}),
        mpack!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::new([OrderByItem::asc("age").nulls_last()]);
    apply_order_by_qv(&mut records, &order);
    assert_eq!(records[0]["age"], 25_i64);
    assert_eq!(records[1]["age"], 30_i64);
    assert!(records[2]["age"].is_null());
}

#[test]
fn order_by_empty_records() {
    let mut records: Vec<QueryValue> = vec![];
    let order = OrderBy::asc("age");
    apply_order_by_qv(&mut records, &order);
    assert!(records.is_empty());
}

#[test]
fn order_by_single_record() {
    let mut records = vec![mpack!({"name": "Alice", "age": 30})];
    let order = OrderBy::desc("age");
    apply_order_by_qv(&mut records, &order);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["age"], 30_i64);
}

#[test]
fn order_by_explicit_null_value() {
    let mut records = vec![
        mpack!({"name": "Alice", "age": 30}),
        mpack!({"name": "Bob", "age": null}),
        mpack!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::asc("age");
    apply_order_by_qv(&mut records, &order);
    // Default ASC -> NullsOrder::Last
    assert_eq!(records[0]["age"], 25_i64);
    assert_eq!(records[1]["age"], 30_i64);
    assert!(records[2]["age"].is_null());
}

#[test]
fn order_by_mixed_types() {
    let mut records = vec![
        mpack!({"val": "hello"}),
        mpack!({"val": 42}),
        mpack!({"val": true}),
    ];

    let order = OrderBy::asc("val");
    apply_order_by_qv(&mut records, &order);
    // Mixed types compare as Equal in the default comparator,
    // so original order is preserved (stable sort).
    assert_eq!(records[0]["val"], "hello");
    assert_eq!(records[1]["val"], 42_i64);
    assert_eq!(records[2]["val"], true);
}

#[test]
fn order_by_empty_items() {
    let mut records = vec![
        mpack!({"name": "Carol"}),
        mpack!({"name": "Alice"}),
        mpack!({"name": "Bob"}),
    ];
    let order = OrderBy::new([]);
    apply_order_by_qv(&mut records, &order);
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
    let records: Vec<QueryValue> = (1..=5).map(QueryValue::Int).collect();

    let pagination = Pagination::LimitOffset {
        limit: Some(2),
        offset: 1,
    };
    let (result, info) = apply_pagination(records, &pagination, true);

    assert_eq!(result, vec![QueryValue::Int(2), QueryValue::Int(3)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert!(info.has_next);
    assert!(info.has_prev);
}

#[test]
fn pagination_page_based() {
    let records: Vec<QueryValue> = (1..=5).map(QueryValue::Int).collect();

    let pagination = Pagination::page(2, 2);
    let (result, info) = apply_pagination(records, &pagination, true);

    assert_eq!(result, vec![QueryValue::Int(3), QueryValue::Int(4)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert_eq!(info.current_page, Some(2));
    assert!(info.has_next);
    assert!(info.has_prev);
}

#[test]
fn pagination_count_total_false() {
    let records: Vec<QueryValue> = (1..=3).map(QueryValue::Int).collect();

    let pagination = Pagination::LimitOffset {
        limit: Some(2),
        offset: 0,
    };
    let (result, info) = apply_pagination(records, &pagination, false);

    assert_eq!(result, vec![QueryValue::Int(1), QueryValue::Int(2)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, None);
}

#[test]
fn pagination_none_no_count() {
    let records: Vec<QueryValue> = (1..=2).map(QueryValue::Int).collect();
    let pagination = Pagination::None;
    let (result, info) = apply_pagination(records, &pagination, false);
    assert_eq!(result, vec![QueryValue::Int(1), QueryValue::Int(2)]);
    assert!(info.is_none());
}

// ============================================================================
// apply_distinct_qv tests (migrated from legacy apply_distinct)
// ============================================================================

#[test]
fn distinct_removes_duplicates() {
    use shamir_types::types::common::new_map_wc;
    fn qv_map(pairs: &[(&str, QueryValue)]) -> QueryValue {
        let mut m = new_map_wc(pairs.len());
        for (k, v) in pairs {
            m.insert((*k).to_string(), v.clone());
        }
        QueryValue::Map(m)
    }

    let records = vec![
        qv_map(&[("a", QueryValue::Int(1))]),
        qv_map(&[("a", QueryValue::Int(2))]),
        qv_map(&[("a", QueryValue::Int(1))]),
        qv_map(&[("a", QueryValue::Int(3))]),
        qv_map(&[("a", QueryValue::Int(2))]),
    ];

    let result = apply_distinct_qv(records);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0]["a"], QueryValue::Int(1));
    assert_eq!(result[1]["a"], QueryValue::Int(2));
    assert_eq!(result[2]["a"], QueryValue::Int(3));
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
