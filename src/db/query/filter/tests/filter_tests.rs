//! Tests for Filter parsing from JSON
//!
//! All tests use JSON strings as source, parse to QueryValue, then convert to Filter.

use crate::db::query::common::{filter_from_value, filter_value_from_value, QueryParseError};
use crate::db::query::filter::{Filter, FilterValue};
use crate::types::value::QueryValue;

/// Parse JSON string to QueryValue, then to Filter
fn parse_filter(json: &str) -> Result<Filter, QueryParseError> {
    let value: QueryValue = serde_json::from_str(json).expect("Invalid JSON");
    filter_from_value(&value)
}

/// Parse JSON string to FilterValue
fn parse_filter_value(json: &str) -> Result<FilterValue, QueryParseError> {
    let value: QueryValue = serde_json::from_str(json).expect("Invalid JSON");
    filter_value_from_value(&value)
}

#[test]
fn test_filter_eq_string() {
    let json = r#"{
        "op": "eq",
        "field": "status",
        "value": "active"
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == "status" && value == FilterValue::String("active".to_string())
    ));
}

#[test]
fn test_filter_eq_integer() {
    let json = r#"{
        "op": "eq",
        "field": "count",
        "value": 42
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == "count" && value == FilterValue::Int(42)
    ));
}

#[test]
fn test_filter_eq_boolean() {
    let json = r#"{
        "op": "eq",
        "field": "active",
        "value": true
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == "active" && value == FilterValue::Bool(true)
    ));
}

#[test]
fn test_filter_eq_null() {
    let json = r#"{
        "op": "eq",
        "field": "deleted_at",
        "value": null
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == "deleted_at" && value == FilterValue::Null
    ));
}

#[test]
fn test_filter_ne() {
    let json = r#"{
        "op": "ne",
        "field": "status",
        "value": "deleted"
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Ne { field, value }
            if field == "status" && value == FilterValue::String("deleted".to_string())
    ));
}

#[test]
fn test_filter_gt() {
    let json = r#"{
        "op": "gt",
        "field": "age",
        "value": 18
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Gt { field, value }
            if field == "age" && value == FilterValue::Int(18)
    ));
}

#[test]
fn test_filter_gte() {
    let json = r#"{
        "op": "gte",
        "field": "salary",
        "value": 50000
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Gte { field, value }
            if field == "salary" && value == FilterValue::Int(50000)
    ));
}

#[test]
fn test_filter_lt() {
    let json = r#"{
        "op": "lt",
        "field": "age",
        "value": 65
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Lt { field, value }
            if field == "age" && value == FilterValue::Int(65)
    ));
}

#[test]
fn test_filter_lte() {
    let json = r#"{
        "op": "lte",
        "field": "stock",
        "value": 100
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(
        filter,
        Filter::Lte { field, value }
            if field == "stock" && value == FilterValue::Int(100)
    ));
}

#[test]
fn test_filter_and() {
    let json = r#"{
        "op": "and",
        "filters": [
            { "op": "eq", "field": "status", "value": "active" },
            { "op": "gt", "field": "age", "value": 18 }
        ]
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_filter_or() {
    let json = r#"{
        "op": "or",
        "filters": [
            { "op": "eq", "field": "role", "value": "admin" },
            { "op": "eq", "field": "role", "value": "moderator" }
        ]
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::Or { filters } if filters.len() == 2));
}

#[test]
fn test_filter_not() {
    let json = r#"{
        "op": "not",
        "filter": {
            "op": "eq",
            "field": "status",
            "value": "deleted"
        }
    }"#;

    let filter = parse_filter(json).unwrap();
    match filter {
        Filter::Not { filter: inner } => {
            assert!(matches!(*inner, Filter::Eq { .. }));
        }
        _ => panic!("Expected Not filter"),
    }
}

#[test]
fn test_filter_is_null() {
    let json = r#"{
        "op": "is_null",
        "field": "deleted_at"
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::IsNull { field } if field == "deleted_at"));
}

#[test]
fn test_filter_is_not_null() {
    let json = r#"{
        "op": "is_not_null",
        "field": "email_verified_at"
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::IsNotNull { field } if field == "email_verified_at"));
}

#[test]
fn test_nested_logical_and_or() {
    let json = r#"{
        "op": "and",
        "filters": [
            {
                "op": "or",
                "filters": [
                    { "op": "eq", "field": "role", "value": "admin" },
                    { "op": "eq", "field": "role", "value": "moderator" }
                ]
            },
            { "op": "eq", "field": "active", "value": true }
        ]
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_nested_logical_three_levels() {
    let json = r#"{
        "op": "and",
        "filters": [
            {
                "op": "or",
                "filters": [
                    { "op": "eq", "field": "status", "value": "active" },
                    { "op": "eq", "field": "status", "value": "pending" }
                ]
            },
            { "op": "gt", "field": "age", "value": 18 },
            {
                "op": "and",
                "filters": [
                    { "op": "eq", "field": "department", "value": "engineering" },
                    { "op": "gte", "field": "salary", "value": 50000 }
                ]
            }
        ]
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 3));
}

#[test]
fn test_not_with_or() {
    let json = r#"{
        "op": "not",
        "filter": {
            "op": "or",
            "filters": [
                { "op": "eq", "field": "status", "value": "banned" },
                { "op": "eq", "field": "status", "value": "deleted" }
            ]
        }
    }"#;

    let filter = parse_filter(json).unwrap();
    match filter {
        Filter::Not { filter: inner } => {
            assert!(matches!(*inner, Filter::Or { .. }));
        }
        _ => panic!("Expected Not filter"),
    }
}

#[test]
fn test_filter_value_types() {
    // String
    let v = parse_filter_value(r#""hello""#).unwrap();
    assert!(matches!(v, FilterValue::String(s) if s == "hello"));

    // Integer
    let v = parse_filter_value("42").unwrap();
    assert!(matches!(v, FilterValue::Int(42)));

    // Float
    let v = parse_filter_value("19.99").unwrap();
    assert!(matches!(v, FilterValue::Float(f) if f == 19.99));

    // Boolean
    let v = parse_filter_value("true").unwrap();
    assert!(matches!(v, FilterValue::Bool(true)));

    // Null
    let v = parse_filter_value("null").unwrap();
    assert!(matches!(v, FilterValue::Null));

    // Array
    let v = parse_filter_value(r#"[1, 2, 3]"#).unwrap();
    assert!(matches!(v, FilterValue::Array(arr) if arr.len() == 3));
}

#[test]
fn test_complex_permission_check() {
    let json = r#"{
        "op": "and",
        "filters": [
            {
                "op": "or",
                "filters": [
                    {
                        "op": "and",
                        "filters": [
                            { "op": "eq", "field": "role", "value": "admin" },
                            { "op": "eq", "field": "active", "value": true }
                        ]
                    },
                    { "op": "eq", "field": "superuser", "value": true }
                ]
            },
            { "op": "gt", "field": "trust_level", "value": 5 }
        ]
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}
