//! Tests for SELECT Query parsing from JSON
//!
//! All tests use JSON strings as source, parse to QueryValue, then convert to Query.

use crate::db::query::filter::Filter;
use crate::db::query::read::query_from_value;
use crate::db::query::read::{ReadQuery, SelectItem};
use crate::types::value::QueryValue;

/// Parse JSON string to QueryValue, then to Query
fn parse_query(json: &str) -> ReadQuery {
    let query_value: QueryValue = serde_json::from_str(json).expect("Invalid JSON");
    query_from_value(&query_value).expect("Failed to parse query")
}

#[test]
fn test_parse_simple_query_from_json() {
    let json = r#"{
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
    }"#;

    let query = parse_query(json);

    assert_eq!(query.from, "users");
    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(query.select.items[0], SelectItem::All));
    assert!(query.r#where.is_some());
    assert_eq!(query.limit.limit, Some(10));
}

#[test]
fn test_parse_select_fields() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                { "type": "field", "path": "name" },
                { "type": "field", "path": "email" },
                { "type": "field", "path": "age" }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 3);
}

#[test]
fn test_parse_select_with_aggregation() {
    let json = r#"{
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
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 3);
}

#[test]
fn test_parse_complex_filter() {
    let json = r#"{
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
    }"#;

    let query = parse_query(json);

    assert!(query.r#where.is_some());
    let filter = query.r#where.unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_parse_deeply_nested_filters() {
    let json = r#"{
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
    }"#;

    let query = parse_query(json);

    assert!(query.r#where.is_some());
    let filter = query.r#where.unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 3));
}
