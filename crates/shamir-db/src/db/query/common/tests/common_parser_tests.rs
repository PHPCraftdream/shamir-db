//! Tests for common query parsers from JSON
//!
//! All tests use JSON strings as source, parse to QueryValue, then convert to structs.

use crate::db::query::common::{
    agg_func_from_str, aggregate_field_from_value, expr_from_value, expr_value_from_value,
    filter_from_value, group_by_from_value, pagination_from_value, order_by_from_value,
    order_by_item_from_value, QueryParseError,
};
use crate::db::query::read::{
    AggFunc, AggregateField, Pagination, SelectExpr, SelectExprValue, NullsOrder, OrderDirection,
};
use crate::types::value::QueryValue;

// ============================================================================
// ORDER BY Tests
// ============================================================================

#[test]
fn test_order_by_single_asc() {
    let json = r#"{
        "items": [
            { "field": "name", "order": "asc" }
        ]
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let order = order_by_from_value(&value).unwrap();

    assert_eq!(order.items.len(), 1);
    assert_eq!(order.items[0].field, vec!["name".to_string()]);
    assert_eq!(order.items[0].direction, OrderDirection::Asc);
    assert!(order.items[0].nulls.is_none());
}

#[test]
fn test_order_by_single_desc() {
    let json = r#"{
        "items": [
            { "field": "created_at", "order": "desc" }
        ]
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let order = order_by_from_value(&value).unwrap();

    assert_eq!(order.items.len(), 1);
    assert_eq!(order.items[0].field, vec!["created_at".to_string()]);
    assert_eq!(order.items[0].direction, OrderDirection::Desc);
}

#[test]
fn test_order_by_with_nulls() {
    let json = r#"{
        "items": [
            { "field": "created_at", "order": "desc", "nulls": "last" },
            { "field": "name", "order": "asc", "nulls": "first" }
        ]
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let order = order_by_from_value(&value).unwrap();

    assert_eq!(order.items.len(), 2);
    assert_eq!(order.items[0].nulls, Some(NullsOrder::Last));
    assert_eq!(order.items[1].nulls, Some(NullsOrder::First));
}

#[test]
fn test_order_by_item_asc() {
    let json = r#"{ "field": "name", "order": "asc" }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let item = order_by_item_from_value(&value).unwrap();
    assert_eq!(item.direction, OrderDirection::Asc);
    assert_eq!(item.field, vec!["name".to_string()]);
}

#[test]
fn test_order_by_item_desc() {
    let json = r#"{ "field": "date", "order": "desc" }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let item = order_by_item_from_value(&value).unwrap();
    assert_eq!(item.direction, OrderDirection::Desc);
    assert_eq!(item.field, vec!["date".to_string()]);
}

// ============================================================================
// PAGINATION Tests (limit/offset)
// ============================================================================

#[test]
fn test_limit_only() {
    let json = r#"{ "limit": 10 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(p, Pagination::LimitOffset { limit: Some(10), offset: 0 });
}

#[test]
fn test_limit_with_offset() {
    let json = r#"{ "limit": 10, "offset": 20 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(p, Pagination::LimitOffset { limit: Some(10), offset: 20 });
}

#[test]
fn test_offset_only() {
    let json = r#"{ "offset": 50 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(p, Pagination::LimitOffset { limit: None, offset: 50 });
}

// ============================================================================
// PAGINATION Tests (page-based)
// ============================================================================

#[test]
fn test_page_based() {
    let json = r#"{ "page": 2, "page_size": 10 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(p, Pagination::Page { page: 2, page_size: 10 });
}

#[test]
fn test_page_based_page_1() {
    let json = r#"{ "page": 1, "page_size": 25 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(p, Pagination::Page { page: 1, page_size: 25 });
}

#[test]
fn test_page_based_missing_page_size() {
    let json = r#"{ "page": 2 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let result = pagination_from_value(&value);
    assert!(result.is_err());
}

#[test]
fn test_not_an_object_pagination() {
    let json = r#""not an object""#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let result = pagination_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::InvalidType(_, _))));
}

// ============================================================================
// GROUP BY Tests
// ============================================================================

#[test]
fn test_group_by_single_field() {
    let json = r#"{ "fields": ["department"] }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let group = group_by_from_value(&value).unwrap();
    assert_eq!(group.fields, vec![vec!["department".to_string()]]);
    assert!(group.having.is_none());
}

#[test]
fn test_group_by_multiple_fields() {
    let json = r#"{ "fields": ["department", "role"] }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let group = group_by_from_value(&value).unwrap();
    assert_eq!(group.fields, vec![vec!["department".to_string()], vec!["role".to_string()]]);
}

#[test]
fn test_group_by_with_having() {
    let json = r#"{
        "fields": ["customer_id"],
        "having": {
            "op": "gt",
            "field": "count",
            "value": 5
        }
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let group = group_by_from_value(&value).unwrap();

    assert_eq!(group.fields, vec![vec!["customer_id".to_string()]]);
    assert!(group.having.is_some());
}

// ============================================================================
// AGGREGATE Tests
// ============================================================================

#[test]
fn test_agg_func_from_str() {
    assert!(matches!(agg_func_from_str("count"), Ok(AggFunc::Count)));
    assert!(matches!(agg_func_from_str("sum"), Ok(AggFunc::Sum)));
    assert!(matches!(agg_func_from_str("avg"), Ok(AggFunc::Avg)));
    assert!(matches!(agg_func_from_str("min"), Ok(AggFunc::Min)));
    assert!(matches!(agg_func_from_str("max"), Ok(AggFunc::Max)));
    assert!(agg_func_from_str("unknown").is_err());
}

#[test]
fn test_aggregate_field_field() {
    let json = r#"{ "type": "field", "name": "salary" }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let field = aggregate_field_from_value(&value).unwrap();
    assert!(matches!(field, AggregateField::Field(name) if name == vec!["salary".to_string()]));
}

#[test]
fn test_aggregate_field_all() {
    let json = r#"{ "type": "all" }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let field = aggregate_field_from_value(&value).unwrap();
    assert!(matches!(field, AggregateField::All));
}

#[test]
fn test_aggregate_field_string() {
    let json = r#""salary""#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let field = aggregate_field_from_value(&value).unwrap();
    assert!(matches!(field, AggregateField::Field(name) if name == vec!["salary".to_string()]));
}

// ============================================================================
// EXPRESSION Tests
// ============================================================================

#[test]
fn test_expr_field() {
    let json = r#"{ "type": "field", "name": "price" }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let expr = expr_from_value(&value).unwrap();
    assert!(matches!(expr, SelectExpr::Field { path } if path == vec!["price".to_string()]));
}

#[test]
fn test_expr_literal_string() {
    let json = r#"{ "type": "literal", "value": "hello" }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let expr = expr_from_value(&value).unwrap();
    assert!(matches!(
        expr,
        SelectExpr::Literal { value: SelectExprValue::String(s) } if s == "hello"
    ));
}

#[test]
fn test_expr_literal_int() {
    let json = r#"{ "type": "literal", "value": 42 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let expr = expr_from_value(&value).unwrap();
    assert!(matches!(
        expr,
        SelectExpr::Literal {
            value: SelectExprValue::Int(42)
        }
    ));
}

#[test]
fn test_expr_literal_bool() {
    let json = r#"{ "type": "literal", "value": true }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let expr = expr_from_value(&value).unwrap();
    assert!(matches!(
        expr,
        SelectExpr::Literal {
            value: SelectExprValue::Bool(true)
        }
    ));
}

#[test]
fn test_expr_literal_null() {
    let json = r#"{ "type": "literal", "value": null }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let expr = expr_from_value(&value).unwrap();
    assert!(matches!(
        expr,
        SelectExpr::Literal {
            value: SelectExprValue::Null
        }
    ));
}

#[test]
fn test_expr_value_types() {
    // Null
    let v: QueryValue = serde_json::from_str("null").unwrap();
    assert!(matches!(expr_value_from_value(&v), Ok(SelectExprValue::Null)));

    // Bool
    let v: QueryValue = serde_json::from_str("true").unwrap();
    assert!(matches!(
        expr_value_from_value(&v),
        Ok(SelectExprValue::Bool(true))
    ));

    // Int
    let v: QueryValue = serde_json::from_str("42").unwrap();
    assert!(matches!(expr_value_from_value(&v), Ok(SelectExprValue::Int(42))));

    // Float
    let v: QueryValue = serde_json::from_str("3.14").unwrap();
    assert!(matches!(expr_value_from_value(&v), Ok(SelectExprValue::Float(_))));

    // String
    let v: QueryValue = serde_json::from_str(r#""hello""#).unwrap();
    assert!(matches!(expr_value_from_value(&v), Ok(SelectExprValue::String(s)) if s == "hello"));
}

// ============================================================================
// ERROR Tests
// ============================================================================

#[test]
fn test_error_not_an_object() {
    let json = r#""not an object""#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let result = filter_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::InvalidType(_, _))));
}

#[test]
fn test_error_missing_field() {
    let json = r#"{ "op": "eq" }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let result = filter_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::MissingField(_))));
}

#[test]
fn test_error_unknown_filter_op() {
    let json = r#"{ "op": "unknown", "field": "x", "value": 1 }"#;
    let value: QueryValue = serde_json::from_str(json).unwrap();
    let result = filter_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::UnknownFilterOp(_))));
}

#[test]
fn test_error_unknown_agg_func() {
    assert!(agg_func_from_str("unknown_func").is_err());
}
