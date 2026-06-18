//! Tests for SELECT Query parsing from QueryValue
//!
//! All tests construct QueryValue directly via mpack! and pass to the parser.

use crate::query::filter::Filter;
use crate::query::read::query_from_value;
use crate::query::read::{ReadQuery, SelectItem};
use crate::query::TableRef;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

/// Construct a ReadQuery from a QueryValue produced by mpack!.
fn parse_query(value: QueryValue) -> ReadQuery {
    query_from_value(&value).expect("Failed to parse query")
}

#[test]
fn test_parse_simple_query_from_value() {
    let query = parse_query(mpack!({
        "from": "users",
        "select": {
            "items": [
                { "type": "all" }
            ],
            "distinct": false
        },
        "where": {
            "op": "eq",
            "field": "status",
            "value": "active"
        },
        "limit": {
            "limit": 10,
            "offset": 0
        }
    }));

    assert_eq!(query.from, TableRef::new("users"));
    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(query.select.items[0], SelectItem::All));
    assert!(query.r#where.is_some());
    assert!(matches!(
        query.pagination,
        crate::query::read::Pagination::LimitOffset {
            limit: Some(10),
            ..
        }
    ));
}

#[test]
fn test_parse_select_fields() {
    let query = parse_query(mpack!({
        "from": "users",
        "select": {
            "items": [
                { "type": "field", "path": "name" },
                { "type": "field", "path": "email" },
                { "type": "field", "path": "age" }
            ]
        }
    }));

    assert_eq!(query.select.items.len(), 3);
}

#[test]
fn test_parse_select_with_aggregation() {
    let query = parse_query(mpack!({
        "from": "users",
        "select": {
            "items": [
                { "type": "field", "path": "department" },
                {
                    "type": "aggregate",
                    "func": "count",
                    "field": { "type": "field", "name": "id" },
                    "alias": "count",
                    "distinct": false
                },
                {
                    "type": "aggregate",
                    "func": "avg",
                    "field": { "type": "field", "name": "salary" },
                    "alias": "avg_salary"
                }
            ]
        }
    }));

    assert_eq!(query.select.items.len(), 3);
}

#[test]
fn test_parse_complex_filter() {
    let query = parse_query(mpack!({
        "from": "users",
        "where": {
            "op": "and",
            "filters": [
                {
                    "op": "eq",
                    "field": "status",
                    "value": "active"
                },
                {
                    "op": "gt",
                    "field": "age",
                    "value": 18
                }
            ]
        }
    }));

    assert!(query.r#where.is_some());
    let filter = query.r#where.unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_parse_deeply_nested_filters() {
    let query = parse_query(mpack!({
        "from": "users",
        "where": {
            "op": "and",
            "filters": [
                {
                    "op": "or",
                    "filters": [
                        {
                            "op": "eq",
                            "field": "status",
                            "value": "active"
                        },
                        {
                            "op": "eq",
                            "field": "status",
                            "value": "pending"
                        }
                    ]
                },
                {
                    "op": "gt",
                    "field": "age",
                    "value": 18
                },
                {
                    "op": "and",
                    "filters": [
                        {
                            "op": "eq",
                            "field": "department",
                            "value": "engineering"
                        },
                        {
                            "op": "gte",
                            "field": "salary",
                            "value": 50000
                        }
                    ]
                }
            ]
        }
    }));

    assert!(query.r#where.is_some());
    let filter = query.r#where.unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 3));
}
