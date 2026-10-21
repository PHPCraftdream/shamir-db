//! apply_pagination / apply_distinct_qv / has_aggregates tests (former
//! `exec_tests.rs`).


use crate::query::read::exec::*;
use crate::query::read::*;
use shamir_query_builder::select;
use shamir_types::types::value::QueryValue;

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

