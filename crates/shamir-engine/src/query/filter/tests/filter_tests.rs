//! Tests for Filter parsing from QueryValue
//!
//! All tests construct QueryValue directly via mpack! and pass to the parsers.

use crate::query::common::{filter_from_value, filter_value_from_value};
use crate::query::filter::{Filter, FilterExprOp, FilterValue};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

#[test]
fn test_filter_eq_string() {
    let filter = filter_from_value(&mpack!({
        "op": "eq",
        "field": "status",
        "value": "active"
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["status".to_string()] && value == FilterValue::String("active".to_string())
    ));
}

#[test]
fn test_filter_eq_integer() {
    let filter = filter_from_value(&mpack!({
        "op": "eq",
        "field": "count",
        "value": 42
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["count".to_string()] && value == FilterValue::Int(42)
    ));
}

#[test]
fn test_filter_eq_boolean() {
    let filter = filter_from_value(&mpack!({
        "op": "eq",
        "field": "active",
        "value": true
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["active".to_string()] && value == FilterValue::Bool(true)
    ));
}

#[test]
fn test_filter_eq_null() {
    let filter = filter_from_value(&mpack!({
        "op": "eq",
        "field": "deleted_at",
        "value": null
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["deleted_at".to_string()] && value == FilterValue::Null
    ));
}

#[test]
fn test_filter_ne() {
    let filter = filter_from_value(&mpack!({
        "op": "ne",
        "field": "status",
        "value": "deleted"
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Ne { field, value }
            if field == vec!["status".to_string()] && value == FilterValue::String("deleted".to_string())
    ));
}

#[test]
fn test_filter_gt() {
    let filter = filter_from_value(&mpack!({
        "op": "gt",
        "field": "age",
        "value": 18
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Gt { field, value }
            if field == vec!["age".to_string()] && value == FilterValue::Int(18)
    ));
}

#[test]
fn test_filter_gte() {
    let filter = filter_from_value(&mpack!({
        "op": "gte",
        "field": "salary",
        "value": 50000
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Gte { field, value }
            if field == vec!["salary".to_string()] && value == FilterValue::Int(50000)
    ));
}

#[test]
fn test_filter_lt() {
    let filter = filter_from_value(&mpack!({
        "op": "lt",
        "field": "age",
        "value": 65
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Lt { field, value }
            if field == vec!["age".to_string()] && value == FilterValue::Int(65)
    ));
}

#[test]
fn test_filter_lte() {
    let filter = filter_from_value(&mpack!({
        "op": "lte",
        "field": "stock",
        "value": 100
    }))
    .unwrap();
    assert!(matches!(
        filter,
        Filter::Lte { field, value }
            if field == vec!["stock".to_string()] && value == FilterValue::Int(100)
    ));
}

#[test]
fn test_filter_and() {
    let filter = filter_from_value(&mpack!({
        "op": "and",
        "filters": [
            { "op": "eq", "field": "status", "value": "active" },
            { "op": "gt", "field": "age", "value": 18 }
        ]
    }))
    .unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_filter_or() {
    let filter = filter_from_value(&mpack!({
        "op": "or",
        "filters": [
            { "op": "eq", "field": "role", "value": "admin" },
            { "op": "eq", "field": "role", "value": "moderator" }
        ]
    }))
    .unwrap();
    assert!(matches!(filter, Filter::Or { filters } if filters.len() == 2));
}

#[test]
fn test_filter_not() {
    let filter = filter_from_value(&mpack!({
        "op": "not",
        "filter": {
            "op": "eq",
            "field": "status",
            "value": "deleted"
        }
    }))
    .unwrap();
    match filter {
        Filter::Not { filter: inner } => {
            assert!(matches!(*inner, Filter::Eq { .. }));
        }
        _ => panic!("Expected Not filter"),
    }
}

#[test]
fn test_filter_is_null() {
    let filter = filter_from_value(&mpack!({
        "op": "is_null",
        "field": "deleted_at"
    }))
    .unwrap();
    assert!(matches!(filter, Filter::IsNull { field } if field == vec!["deleted_at".to_string()]));
}

#[test]
fn test_filter_is_not_null() {
    let filter = filter_from_value(&mpack!({
        "op": "is_not_null",
        "field": "email_verified_at"
    }))
    .unwrap();
    assert!(
        matches!(filter, Filter::IsNotNull { field } if field == vec!["email_verified_at".to_string()])
    );
}

#[test]
fn test_nested_logical_and_or() {
    let filter = filter_from_value(&mpack!({
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
    }))
    .unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_nested_logical_three_levels() {
    let filter = filter_from_value(&mpack!({
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
    }))
    .unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 3));
}

#[test]
fn test_not_with_or() {
    let filter = filter_from_value(&mpack!({
        "op": "not",
        "filter": {
            "op": "or",
            "filters": [
                { "op": "eq", "field": "status", "value": "banned" },
                { "op": "eq", "field": "status", "value": "deleted" }
            ]
        }
    }))
    .unwrap();
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
    let v = filter_value_from_value(&QueryValue::Str("hello".to_string())).unwrap();
    assert!(matches!(v, FilterValue::String(s) if s == "hello"));

    // Integer
    let v = filter_value_from_value(&QueryValue::Int(42)).unwrap();
    assert!(matches!(v, FilterValue::Int(42)));

    // Float
    let v = filter_value_from_value(&QueryValue::F64(19.99)).unwrap();
    assert!(matches!(v, FilterValue::Float(f) if f == 19.99));

    // Boolean
    let v = filter_value_from_value(&QueryValue::Bool(true)).unwrap();
    assert!(matches!(v, FilterValue::Bool(true)));

    // Null
    let v = filter_value_from_value(&QueryValue::Null).unwrap();
    assert!(matches!(v, FilterValue::Null));

    // Array
    let v = filter_value_from_value(&mpack!([1, 2, 3])).unwrap();
    assert!(matches!(v, FilterValue::Array(arr) if arr.len() == 3));
}

#[test]
fn test_complex_permission_check() {
    let filter = filter_from_value(&mpack!({
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
    }))
    .unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

// ============================================================================
// Field Reference Tests ($ref)
// ============================================================================

#[test]
fn test_filter_value_field_ref() {
    let v = filter_value_from_value(&mpack!({ "$ref": "address.city" })).unwrap();
    assert!(
        matches!(v, FilterValue::FieldRef { path } if path == vec!["address".to_string(), "city".to_string()])
    );
}

#[test]
fn test_filter_value_field_ref_nested() {
    let v = filter_value_from_value(&mpack!({ "$ref": "user.profile.bio" })).unwrap();
    assert!(
        matches!(v, FilterValue::FieldRef { path } if path == vec!["user".to_string(), "profile".to_string(), "bio".to_string()])
    );
}

#[test]
fn test_filter_eq_with_field_ref() {
    let filter = filter_from_value(&mpack!({
        "op": "eq",
        "field": "billing_city",
        "value": { "$ref": "address.city" }
    }))
    .unwrap();
    match filter {
        Filter::Eq { field, value } => {
            assert_eq!(field, vec!["billing_city".to_string()]);
            assert!(
                matches!(value, FilterValue::FieldRef { path } if path == vec!["address".to_string(), "city".to_string()])
            );
        }
        _ => panic!("Expected Eq filter"),
    }
}

#[test]
fn test_filter_gt_with_field_ref() {
    let filter = filter_from_value(&mpack!({
        "op": "gt",
        "field": "end_date",
        "value": { "$ref": "start_date" }
    }))
    .unwrap();
    match filter {
        Filter::Gt { field, value } => {
            assert_eq!(field, vec!["end_date".to_string()]);
            assert!(
                matches!(value, FilterValue::FieldRef { path } if path == vec!["start_date".to_string()])
            );
        }
        _ => panic!("Expected Gt filter"),
    }
}

#[test]
fn test_filter_with_mixed_values() {
    let filter = filter_from_value(&mpack!({
        "op": "and",
        "filters": [
            { "op": "eq", "field": "status", "value": "active" },
            { "op": "gte", "field": "salary", "value": { "$ref": "min_salary" } }
        ]
    }))
    .unwrap();
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_filter_value_array_with_field_refs() {
    let v = filter_value_from_value(&mpack!([
        { "$ref": "user.id" },
        42,
        "literal"
    ]))
    .unwrap();
    match v {
        FilterValue::Array(arr) => {
            assert_eq!(arr.len(), 3);
            assert!(
                matches!(&arr[0], FilterValue::FieldRef { path } if *path == vec!["user".to_string(), "id".to_string()])
            );
            assert!(matches!(&arr[1], FilterValue::Int(42)));
            assert!(matches!(&arr[2], FilterValue::String(s) if s == "literal"));
        }
        _ => panic!("Expected Array"),
    }
}

#[test]
fn test_field_ref_helper() {
    let v = FilterValue::field_ref("address.city");
    assert!(
        matches!(v, FilterValue::FieldRef { path } if path == vec!["address.city".to_string()])
    );
}

// ============================================================================
// System Function Tests ($fn)
// ============================================================================

#[test]
fn test_fn_call_simple() {
    let v = filter_value_from_value(&mpack!({ "$fn": "NOW" })).unwrap();
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
    let v = filter_value_from_value(&mpack!({
        "$fn": {
            "name": "COALESCE",
            "args": [null, "default"]
        }
    }))
    .unwrap();
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
    let filter = filter_from_value(&mpack!({
        "op": "gte",
        "field": "created_at",
        "value": { "$fn": "NOW" }
    }))
    .unwrap();
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
    let v =
        filter_value_from_value(&mpack!({ "$expr": { "op": "add", "args": [10, 20] } })).unwrap();
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
    let v = filter_value_from_value(&mpack!({
        "$expr": {
            "op": "mul",
            "args": [{ "$ref": "price" }, 1.1]
        }
    }))
    .unwrap();
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
    let v = filter_value_from_value(&mpack!({
        "$expr": {
            "op": "concat",
            "args": [{ "$ref": "first" }, " ", { "$ref": "last" }]
        }
    }))
    .unwrap();
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
    let filter = filter_from_value(&mpack!({
        "op": "gt",
        "field": "total",
        "value": {
            "$expr": {
                "op": "mul",
                "args": [{ "$ref": "price" }, { "$ref": "quantity" }]
            }
        }
    }))
    .unwrap();
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
    let v = filter_value_from_value(&mpack!({
        "$cond": {
            "if": { "op": "eq", "field": "active", "value": true },
            "then": "yes",
            "else": "no"
        }
    }))
    .unwrap();
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
    let v = filter_value_from_value(&mpack!({
        "$cond": {
            "if": { "op": "gte", "field": "score", "value": 100 },
            "then": { "$expr": { "op": "mul", "args": [{ "$ref": "score" }, 2] } },
            "else": { "$ref": "score" }
        }
    }))
    .unwrap();
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
    let v = filter_value_from_value(&mpack!({
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
    }))
    .unwrap();
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
    let filter = filter_from_value(&mpack!({
        "op": "eq",
        "field": "tier",
        "value": {
            "$cond": {
                "if": { "op": "gte", "field": "score", "value": 100 },
                "then": "vip",
                "else": "regular"
            }
        }
    }))
    .unwrap();
    match filter {
        Filter::Eq { field, value } => {
            assert_eq!(field, vec!["tier".to_string()]);
            assert!(matches!(value, FilterValue::Cond { .. }));
        }
        _ => panic!("Expected Eq filter"),
    }
}
