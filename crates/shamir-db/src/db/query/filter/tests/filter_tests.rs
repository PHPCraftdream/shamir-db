//! Tests for Filter parsing from JSON
//!
//! All tests use JSON strings as source, parse to QueryValue, then convert to Filter.

use crate::db::query::common::{filter_from_value, filter_value_from_value, QueryParseError};
use crate::db::query::filter::{FilterExprOp, Filter, FilterValue};
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
            if field == vec!["status".to_string()] && value == FilterValue::String("active".to_string())
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
            if field == vec!["count".to_string()] && value == FilterValue::Int(42)
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
            if field == vec!["active".to_string()] && value == FilterValue::Bool(true)
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
            if field == vec!["deleted_at".to_string()] && value == FilterValue::Null
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
            if field == vec!["status".to_string()] && value == FilterValue::String("deleted".to_string())
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
            if field == vec!["age".to_string()] && value == FilterValue::Int(18)
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
            if field == vec!["salary".to_string()] && value == FilterValue::Int(50000)
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
            if field == vec!["age".to_string()] && value == FilterValue::Int(65)
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
            if field == vec!["stock".to_string()] && value == FilterValue::Int(100)
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
    assert!(matches!(filter, Filter::IsNull { field } if field == vec!["deleted_at".to_string()]));
}

#[test]
fn test_filter_is_not_null() {
    let json = r#"{
        "op": "is_not_null",
        "field": "email_verified_at"
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::IsNotNull { field } if field == vec!["email_verified_at".to_string()]));
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

// ============================================================================
// Field Reference Tests ($ref)
// ============================================================================

#[test]
fn test_filter_value_field_ref() {
    let json = r#"{ "$ref": "address.city" }"#;
    let v = parse_filter_value(json).unwrap();
    assert!(matches!(v, FilterValue::FieldRef { path } if path == vec!["address".to_string(), "city".to_string()]));
}

#[test]
fn test_filter_value_field_ref_nested() {
    let json = r#"{ "$ref": "user.profile.bio" }"#;
    let v = parse_filter_value(json).unwrap();
    assert!(matches!(v, FilterValue::FieldRef { path } if path == vec!["user".to_string(), "profile".to_string(), "bio".to_string()]));
}

#[test]
fn test_filter_eq_with_field_ref() {
    let json = r#"{
        "op": "eq",
        "field": "billing_city",
        "value": { "$ref": "address.city" }
    }"#;

    let filter = parse_filter(json).unwrap();
    match filter {
        Filter::Eq { field, value } => {
            assert_eq!(field, vec!["billing_city".to_string()]);
            assert!(matches!(value, FilterValue::FieldRef { path } if path == vec!["address".to_string(), "city".to_string()]));
        }
        _ => panic!("Expected Eq filter"),
    }
}

#[test]
fn test_filter_gt_with_field_ref() {
    let json = r#"{
        "op": "gt",
        "field": "end_date",
        "value": { "$ref": "start_date" }
    }"#;

    let filter = parse_filter(json).unwrap();
    match filter {
        Filter::Gt { field, value } => {
            assert_eq!(field, vec!["end_date".to_string()]);
            assert!(matches!(value, FilterValue::FieldRef { path } if path == vec!["start_date".to_string()]));
        }
        _ => panic!("Expected Gt filter"),
    }
}

#[test]
fn test_filter_with_mixed_values() {
    // Mix of literal and field reference
    let json = r#"{
        "op": "and",
        "filters": [
            { "op": "eq", "field": "status", "value": "active" },
            { "op": "gte", "field": "salary", "value": { "$ref": "min_salary" } }
        ]
    }"#;

    let filter = parse_filter(json).unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_filter_value_array_with_field_refs() {
    let json = r#"[
        { "$ref": "user.id" },
        42,
        "literal"
    ]"#;

    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::Array(arr) => {
            assert_eq!(arr.len(), 3);
            assert!(matches!(&arr[0], FilterValue::FieldRef { path } if *path == vec!["user".to_string(), "id".to_string()]));
            assert!(matches!(&arr[1], FilterValue::Int(42)));
            assert!(matches!(&arr[2], FilterValue::String(s) if s == "literal"));
        }
        _ => panic!("Expected Array"),
    }
}

#[test]
fn test_field_ref_helper() {
    let v = FilterValue::field_ref("address.city");
    assert!(matches!(v, FilterValue::FieldRef { path } if path == vec!["address.city".to_string()]));
}

// ============================================================================
// System Function Tests ($fn)
// ============================================================================

#[test]
fn test_fn_call_simple() {
    let json = r#"{ "$fn": "NOW" }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::FnCall { call } => {
            assert_eq!(call.name(), "NOW");
            assert!(call.args().is_empty());
        }
        _ => panic!("Expected FnCall"),
    }
}

#[test]
fn test_fn_call_complex_with_args() {
    let json = r#"{
        "$fn": {
            "name": "COALESCE",
            "args": [null, "default"]
        }
    }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::FnCall { call } => {
            assert_eq!(call.name(), "COALESCE");
            assert_eq!(call.args().len(), 2);
        }
        _ => panic!("Expected FnCall"),
    }
}

#[test]
fn test_fn_call_in_filter() {
    let json = r#"{
        "op": "gte",
        "field": "created_at",
        "value": { "$fn": "NOW" }
    }"#;
    let filter = parse_filter(json).unwrap();
    match filter {
        Filter::Gte { field, value } => {
            assert_eq!(field, vec!["created_at".to_string()]);
            assert!(matches!(value, FilterValue::FnCall { .. }));
        }
        _ => panic!("Expected Gte filter"),
    }
}

// ============================================================================
// Expression Tests ($expr)
// ============================================================================

#[test]
fn test_expr_add() {
    let json = r#"{ "$expr": { "op": "add", "args": [10, 20] } }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::Expr { expr } => {
            assert!(matches!(expr.op, FilterExprOp::Add));
            assert_eq!(expr.args.len(), 2);
        }
        _ => panic!("Expected Expr"),
    }
}

#[test]
fn test_expr_mul_with_field_ref() {
    let json = r#"{
        "$expr": {
            "op": "mul",
            "args": [{ "$ref": "price" }, 1.1]
        }
    }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::Expr { expr } => {
            assert!(matches!(expr.op, FilterExprOp::Mul));
            assert_eq!(expr.args.len(), 2);
        }
        _ => panic!("Expected Expr"),
    }
}

#[test]
fn test_expr_concat() {
    let json = r#"{
        "$expr": {
            "op": "concat",
            "args": [{ "$ref": "first" }, " ", { "$ref": "last" }]
        }
    }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::Expr { expr } => {
            assert!(matches!(expr.op, FilterExprOp::Concat));
            assert_eq!(expr.args.len(), 3);
        }
        _ => panic!("Expected Expr"),
    }
}

#[test]
fn test_expr_in_filter() {
    let json = r#"{
        "op": "gt",
        "field": "total",
        "value": {
            "$expr": {
                "op": "mul",
                "args": [{ "$ref": "price" }, { "$ref": "quantity" }]
            }
        }
    }"#;
    let filter = parse_filter(json).unwrap();
    match filter {
        Filter::Gt { field, value } => {
            assert_eq!(field, vec!["total".to_string()]);
            assert!(matches!(value, FilterValue::Expr { .. }));
        }
        _ => panic!("Expected Gt filter"),
    }
}

// ============================================================================
// Conditional Tests ($cond)
// ============================================================================

#[test]
fn test_cond_simple() {
    let json = r#"{
        "$cond": {
            "if": { "op": "eq", "field": "active", "value": true },
            "then": "yes",
            "else": "no"
        }
    }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::Cond { cond } => {
            assert!(matches!(*cond.condition, Filter::Eq { .. }));
            assert!(matches!(cond.then, FilterValue::String(ref s) if s == "yes"));
            assert!(matches!(cond.or_else, FilterValue::String(ref s) if s == "no"));
        }
        _ => panic!("Expected Cond"),
    }
}

#[test]
fn test_cond_with_expr_in_branches() {
    let json = r#"{
        "$cond": {
            "if": { "op": "gte", "field": "score", "value": 100 },
            "then": { "$expr": { "op": "mul", "args": [{ "$ref": "score" }, 2] } },
            "else": { "$ref": "score" }
        }
    }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::Cond { cond } => {
            assert!(matches!(*cond.condition, Filter::Gte { .. }));
            assert!(matches!(cond.then, FilterValue::Expr { .. }));
            assert!(matches!(cond.or_else, FilterValue::FieldRef { .. }));
        }
        _ => panic!("Expected Cond"),
    }
}

#[test]
fn test_cond_nested() {
    let json = r#"{
        "$cond": {
            "if": { "op": "gte", "field": "score", "value": 100 },
            "then": "vip",
            "else": {
                "$cond": {
                    "if": { "op": "gte", "field": "score", "value": 50 },
                    "then": "regular",
                    "else": "newbie"
                }
            }
        }
    }"#;
    let v = parse_filter_value(json).unwrap();
    match v {
        FilterValue::Cond { cond } => {
            assert!(matches!(*cond.condition, Filter::Gte { .. }));
            assert!(matches!(cond.then, FilterValue::String(s) if s == "vip"));
            assert!(matches!(cond.or_else, FilterValue::Cond { .. }));
        }
        _ => panic!("Expected Cond"),
    }
}

#[test]
fn test_cond_in_filter() {
    let json = r#"{
        "op": "eq",
        "field": "tier",
        "value": {
            "$cond": {
                "if": { "op": "gte", "field": "score", "value": 100 },
                "then": "vip",
                "else": "regular"
            }
        }
    }"#;
    let filter = parse_filter(json).unwrap();
    match filter {
        Filter::Eq { field, value } => {
            assert_eq!(field, vec!["tier".to_string()]);
            assert!(matches!(value, FilterValue::Cond { .. }));
        }
        _ => panic!("Expected Eq filter"),
    }
}
