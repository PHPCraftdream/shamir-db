//! Tests for Filter/FilterValue's canonical wire format (`QueryValue` <-> msgpack <-> typed enum).
//!
//! This file used to route through the hand-written `filter_from_value` /
//! `filter_value_from_value` parser (`query::common::parser`, deleted —
//! F-group-5, #1078): a duplicate, divergent re-implementation of what
//! `Filter`'s `#[serde(tag = "op", rename_all = "snake_case")]` and
//! `FilterValue`'s `$ref`/`$fn`/`$expr`/`$cond`/`$query` tags already do via
//! serde. These tests now go through the same `rmp_serde` round-trip the
//! production wire path uses (`qv_to::<T>` in
//! `shamir-query-types::batch::batch_op`; `FilterValue::from(QueryValue)`'s
//! Tier 2 fallback) — this file's subject IS the wire format, so a raw
//! `QueryValue` round-trip is the correct test shape (documented
//! serde-round-trip exception to the builder-only rule), not a builder.
//!
//! Note: the canonical wire shape for `$ref` is always an ARRAY of path
//! segments (`{"$ref": ["a", "b"]}`), never a dot-joined string — the old
//! hand-parser's dot-string leniency was part of the dead dialect and is not
//! reproduced here.

use crate::query::filter::{Filter, FilterExprOp, FilterValue};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

/// Deserialize a `Filter` from its canonical `QueryValue` wire shape via the
/// same msgpack round-trip the production wire path uses.
fn filter_from_qv(value: &QueryValue) -> Filter {
    let bytes = rmp_serde::to_vec_named(value).expect("serialize QueryValue");
    rmp_serde::from_slice(&bytes).expect("deserialize Filter")
}

/// Deserialize a `FilterValue` from its canonical `QueryValue` wire shape.
fn filter_value_from_qv(value: &QueryValue) -> FilterValue {
    let bytes = rmp_serde::to_vec_named(value).expect("serialize QueryValue");
    rmp_serde::from_slice(&bytes).expect("deserialize FilterValue")
}

#[test]
fn test_filter_eq_string() {
    let filter = filter_from_qv(&mpack!({
        "op": "eq",
        "field": "status",
        "value": "active"
    }));
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["status".to_string()] && value == FilterValue::String("active".to_string())
    ));
}

#[test]
fn test_filter_eq_integer() {
    let filter = filter_from_qv(&mpack!({
        "op": "eq",
        "field": "count",
        "value": 42
    }));
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["count".to_string()] && value == FilterValue::Int(42)
    ));
}

#[test]
fn test_filter_eq_boolean() {
    let filter = filter_from_qv(&mpack!({
        "op": "eq",
        "field": "active",
        "value": true
    }));
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["active".to_string()] && value == FilterValue::Bool(true)
    ));
}

#[test]
fn test_filter_eq_null() {
    let filter = filter_from_qv(&mpack!({
        "op": "eq",
        "field": "deleted_at",
        "value": null
    }));
    assert!(matches!(
        filter,
        Filter::Eq { field, value }
            if field == vec!["deleted_at".to_string()] && value == FilterValue::Null
    ));
}

#[test]
fn test_filter_ne() {
    let filter = filter_from_qv(&mpack!({
        "op": "ne",
        "field": "status",
        "value": "deleted"
    }));
    assert!(matches!(
        filter,
        Filter::Ne { field, value }
            if field == vec!["status".to_string()] && value == FilterValue::String("deleted".to_string())
    ));
}

#[test]
fn test_filter_gt() {
    let filter = filter_from_qv(&mpack!({
        "op": "gt",
        "field": "age",
        "value": 18
    }));
    assert!(matches!(
        filter,
        Filter::Gt { field, value }
            if field == vec!["age".to_string()] && value == FilterValue::Int(18)
    ));
}

#[test]
fn test_filter_gte() {
    let filter = filter_from_qv(&mpack!({
        "op": "gte",
        "field": "salary",
        "value": 50000
    }));
    assert!(matches!(
        filter,
        Filter::Gte { field, value }
            if field == vec!["salary".to_string()] && value == FilterValue::Int(50000)
    ));
}

#[test]
fn test_filter_lt() {
    let filter = filter_from_qv(&mpack!({
        "op": "lt",
        "field": "age",
        "value": 65
    }));
    assert!(matches!(
        filter,
        Filter::Lt { field, value }
            if field == vec!["age".to_string()] && value == FilterValue::Int(65)
    ));
}

#[test]
fn test_filter_lte() {
    let filter = filter_from_qv(&mpack!({
        "op": "lte",
        "field": "stock",
        "value": 100
    }));
    assert!(matches!(
        filter,
        Filter::Lte { field, value }
            if field == vec!["stock".to_string()] && value == FilterValue::Int(100)
    ));
}

#[test]
fn test_filter_and() {
    let filter = filter_from_qv(&mpack!({
        "op": "and",
        "filters": [
            { "op": "eq", "field": "status", "value": "active" },
            { "op": "gt", "field": "age", "value": 18 }
        ]
    }));
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_filter_or() {
    let filter = filter_from_qv(&mpack!({
        "op": "or",
        "filters": [
            { "op": "eq", "field": "role", "value": "admin" },
            { "op": "eq", "field": "role", "value": "moderator" }
        ]
    }));
    assert!(matches!(filter, Filter::Or { filters } if filters.len() == 2));
}

#[test]
fn test_filter_not() {
    let filter = filter_from_qv(&mpack!({
        "op": "not",
        "filter": {
            "op": "eq",
            "field": "status",
            "value": "deleted"
        }
    }));
    match filter {
        Filter::Not { filter: inner } => {
            assert!(matches!(*inner, Filter::Eq { .. }));
        }
        _ => panic!("Expected Not filter"),
    }
}

#[test]
fn test_filter_is_null() {
    let filter = filter_from_qv(&mpack!({
        "op": "is_null",
        "field": "deleted_at"
    }));
    assert!(matches!(filter, Filter::IsNull { field } if field == vec!["deleted_at".to_string()]));
}

#[test]
fn test_filter_is_not_null() {
    let filter = filter_from_qv(&mpack!({
        "op": "is_not_null",
        "field": "email_verified_at"
    }));
    assert!(
        matches!(filter, Filter::IsNotNull { field } if field == vec!["email_verified_at".to_string()])
    );
}

#[test]
fn test_nested_logical_and_or() {
    let filter = filter_from_qv(&mpack!({
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
    }));
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_nested_logical_three_levels() {
    let filter = filter_from_qv(&mpack!({
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
    }));
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 3));
}

#[test]
fn test_not_with_or() {
    let filter = filter_from_qv(&mpack!({
        "op": "not",
        "filter": {
            "op": "or",
            "filters": [
                { "op": "eq", "field": "status", "value": "banned" },
                { "op": "eq", "field": "status", "value": "deleted" }
            ]
        }
    }));
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
    let v = filter_value_from_qv(&QueryValue::Str("hello".to_string()));
    assert!(matches!(v, FilterValue::String(s) if s == "hello"));

    // Integer
    let v = filter_value_from_qv(&QueryValue::Int(42));
    assert!(matches!(v, FilterValue::Int(42)));

    // Float
    let v = filter_value_from_qv(&QueryValue::F64(19.99));
    assert!(matches!(v, FilterValue::Float(f) if f == 19.99));

    // Boolean
    let v = filter_value_from_qv(&QueryValue::Bool(true));
    assert!(matches!(v, FilterValue::Bool(true)));

    // Null
    let v = filter_value_from_qv(&QueryValue::Null);
    assert!(matches!(v, FilterValue::Null));

    // Array
    let v = filter_value_from_qv(&mpack!([1, 2, 3]));
    assert!(matches!(v, FilterValue::Array(arr) if arr.len() == 3));
}

#[test]
fn test_complex_permission_check() {
    let filter = filter_from_qv(&mpack!({
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
    }));
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

// ============================================================================
// Field Reference Tests ($ref)
// ============================================================================

#[test]
fn test_filter_value_field_ref() {
    let v = filter_value_from_qv(&mpack!({ "$ref": ["address", "city"] }));
    assert!(
        matches!(v, FilterValue::FieldRef { path } if path == vec!["address".to_string(), "city".to_string()])
    );
}

#[test]
fn test_filter_value_field_ref_nested() {
    let v = filter_value_from_qv(&mpack!({ "$ref": ["user", "profile", "bio"] }));
    assert!(
        matches!(v, FilterValue::FieldRef { path } if path == vec!["user".to_string(), "profile".to_string(), "bio".to_string()])
    );
}

#[test]
fn test_filter_eq_with_field_ref() {
    let filter = filter_from_qv(&mpack!({
        "op": "eq",
        "field": "billing_city",
        "value": { "$ref": ["address", "city"] }
    }));
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
    let filter = filter_from_qv(&mpack!({
        "op": "gt",
        "field": "end_date",
        "value": { "$ref": ["start_date"] }
    }));
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
    let filter = filter_from_qv(&mpack!({
        "op": "and",
        "filters": [
            { "op": "eq", "field": "status", "value": "active" },
            { "op": "gte", "field": "salary", "value": { "$ref": ["min_salary"] } }
        ]
    }));
    assert!(matches!(filter, Filter::And { filters } if filters.len() == 2));
}

#[test]
fn test_filter_value_array_with_field_refs() {
    let v = filter_value_from_qv(&mpack!([
        { "$ref": ["user", "id"] },
        42,
        "literal"
    ]));
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
    let v = filter_value_from_qv(&mpack!({ "$fn": "NOW" }));
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
    let v = filter_value_from_qv(&mpack!({
        "$fn": {
            "name": "COALESCE",
            "args": [null, "default"]
        }
    }));
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
    let filter = filter_from_qv(&mpack!({
        "op": "gte",
        "field": "created_at",
        "value": { "$fn": "NOW" }
    }));
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
    let v = filter_value_from_qv(&mpack!({ "$expr": { "op": "add", "args": [10, 20] } }));
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
    let v = filter_value_from_qv(&mpack!({
        "$expr": {
            "op": "mul",
            "args": [{ "$ref": ["price"] }, 1.1]
        }
    }));
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
    let v = filter_value_from_qv(&mpack!({
        "$expr": {
            "op": "concat",
            "args": [{ "$ref": ["first"] }, " ", { "$ref": ["last"] }]
        }
    }));
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
    let filter = filter_from_qv(&mpack!({
        "op": "gt",
        "field": "total",
        "value": {
            "$expr": {
                "op": "mul",
                "args": [{ "$ref": ["price"] }, { "$ref": ["quantity"] }]
            }
        }
    }));
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
    let v = filter_value_from_qv(&mpack!({
        "$cond": {
            "if": { "op": "eq", "field": "active", "value": true },
            "then": "yes",
            "else": "no"
        }
    }));
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
    let v = filter_value_from_qv(&mpack!({
        "$cond": {
            "if": { "op": "gte", "field": "score", "value": 100 },
            "then": { "$expr": { "op": "mul", "args": [{ "$ref": ["score"] }, 2] } },
            "else": { "$ref": ["score"] }
        }
    }));
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
    let v = filter_value_from_qv(&mpack!({
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
    }));
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
    let filter = filter_from_qv(&mpack!({
        "op": "eq",
        "field": "tier",
        "value": {
            "$cond": {
                "if": { "op": "gte", "field": "score", "value": 100 },
                "then": "vip",
                "else": "regular"
            }
        }
    }));
    match filter {
        Filter::Eq { field, value } => {
            assert_eq!(field, vec!["tier".to_string()]);
            assert!(matches!(value, FilterValue::Cond { .. }));
        }
        _ => panic!("Expected Eq filter"),
    }
}
