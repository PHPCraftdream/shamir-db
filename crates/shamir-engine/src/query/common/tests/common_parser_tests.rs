//! Tests for common query parsers from QueryValue
//!
//! All tests construct QueryValue directly via mpack! and pass to the parsers.

use crate::query::common::{
    agg_func_from_str, aggregate_field_from_value, expr_from_value, expr_value_from_value,
    filter_from_value, group_by_from_value, order_by_from_value, order_by_item_from_value,
    pagination_from_value, QueryParseError,
};
use crate::query::read::{
    AggFunc, AggregateField, NullsOrder, OrderDirection, Pagination, SelectExpr, SelectExprValue,
};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

// ============================================================================
// ORDER BY Tests
// ============================================================================

#[test]
fn test_order_by_single_asc() {
    let value = mpack!({
        "items": [
            { "field": "name", "order": "asc" }
        ]
    });
    let order = order_by_from_value(&value).unwrap();

    assert_eq!(order.items.len(), 1);
    assert_eq!(order.items[0].field, vec!["name".to_string()]);
    assert_eq!(order.items[0].direction, OrderDirection::Asc);
    assert!(order.items[0].nulls.is_none());
}

#[test]
fn test_order_by_single_desc() {
    let value = mpack!({
        "items": [
            { "field": "created_at", "order": "desc" }
        ]
    });
    let order = order_by_from_value(&value).unwrap();

    assert_eq!(order.items.len(), 1);
    assert_eq!(order.items[0].field, vec!["created_at".to_string()]);
    assert_eq!(order.items[0].direction, OrderDirection::Desc);
}

#[test]
fn test_order_by_with_nulls() {
    let value = mpack!({
        "items": [
            { "field": "created_at", "order": "desc", "nulls": "last" },
            { "field": "name", "order": "asc", "nulls": "first" }
        ]
    });
    let order = order_by_from_value(&value).unwrap();

    assert_eq!(order.items.len(), 2);
    assert_eq!(order.items[0].nulls, Some(NullsOrder::Last));
    assert_eq!(order.items[1].nulls, Some(NullsOrder::First));
}

#[test]
fn test_order_by_item_asc() {
    let value = mpack!({ "field": "name", "order": "asc" });
    let item = order_by_item_from_value(&value).unwrap();
    assert_eq!(item.direction, OrderDirection::Asc);
    assert_eq!(item.field, vec!["name".to_string()]);
}

#[test]
fn test_order_by_item_desc() {
    let value = mpack!({ "field": "date", "order": "desc" });
    let item = order_by_item_from_value(&value).unwrap();
    assert_eq!(item.direction, OrderDirection::Desc);
    assert_eq!(item.field, vec!["date".to_string()]);
}

// ============================================================================
// PAGINATION Tests (limit/offset)
// ============================================================================

#[test]
fn test_limit_only() {
    let value = mpack!({ "limit": 10 });
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(
        p,
        Pagination::LimitOffset {
            limit: Some(10),
            offset: 0
        }
    );
}

#[test]
fn test_limit_with_offset() {
    let value = mpack!({ "limit": 10, "offset": 20 });
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(
        p,
        Pagination::LimitOffset {
            limit: Some(10),
            offset: 20
        }
    );
}

#[test]
fn test_offset_only() {
    let value = mpack!({ "offset": 50 });
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(
        p,
        Pagination::LimitOffset {
            limit: None,
            offset: 50
        }
    );
}

// ============================================================================
// PAGINATION Tests (page-based)
// ============================================================================

#[test]
fn test_page_based() {
    let value = mpack!({ "page": 2, "page_size": 10 });
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(
        p,
        Pagination::Page {
            page: 2,
            page_size: 10
        }
    );
}

#[test]
fn test_page_based_page_1() {
    let value = mpack!({ "page": 1, "page_size": 25 });
    let p = pagination_from_value(&value).unwrap();
    assert_eq!(
        p,
        Pagination::Page {
            page: 1,
            page_size: 25
        }
    );
}

#[test]
fn test_page_based_missing_page_size() {
    let value = mpack!({ "page": 2 });
    let result = pagination_from_value(&value);
    assert!(result.is_err());
}

#[test]
fn test_not_an_object_pagination() {
    let value = QueryValue::Str("not an object".to_string());
    let result = pagination_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::InvalidType(_, _))));
}

// ============================================================================
// GROUP BY Tests
// ============================================================================

#[test]
fn test_group_by_single_field() {
    let value = mpack!({ "fields": ["department"] });
    let group = group_by_from_value(&value).unwrap();
    assert_eq!(group.fields, vec![vec!["department".to_string()]]);
    assert!(group.having.is_none());
}

#[test]
fn test_group_by_multiple_fields() {
    let value = mpack!({ "fields": ["department", "role"] });
    let group = group_by_from_value(&value).unwrap();
    assert_eq!(
        group.fields,
        vec![vec!["department".to_string()], vec!["role".to_string()]]
    );
}

#[test]
fn test_group_by_with_having() {
    let value = mpack!({
        "fields": ["customer_id"],
        "having": {
            "op": "gt",
            "field": "count",
            "value": 5
        }
    });
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
    let value = mpack!({ "type": "field", "name": "salary" });
    let field = aggregate_field_from_value(&value).unwrap();
    assert!(matches!(field, AggregateField::Field(name) if name == vec!["salary".to_string()]));
}

#[test]
fn test_aggregate_field_all() {
    let value = mpack!({ "type": "all" });
    let field = aggregate_field_from_value(&value).unwrap();
    assert!(matches!(field, AggregateField::All));
}

#[test]
fn test_aggregate_field_string() {
    let value = QueryValue::Str("salary".to_string());
    let field = aggregate_field_from_value(&value).unwrap();
    assert!(matches!(field, AggregateField::Field(name) if name == vec!["salary".to_string()]));
}

// ============================================================================
// EXPRESSION Tests
// ============================================================================

#[test]
fn test_expr_field() {
    let value = mpack!({ "type": "field", "name": "price" });
    let expr = expr_from_value(&value).unwrap();
    assert!(matches!(expr, SelectExpr::Field { path } if path == vec!["price".to_string()]));
}

#[test]
fn test_expr_literal_string() {
    let value = mpack!({ "type": "literal", "value": "hello" });
    let expr = expr_from_value(&value).unwrap();
    assert!(matches!(
        expr,
        SelectExpr::Literal { value: SelectExprValue::String(s) } if s == "hello"
    ));
}

#[test]
fn test_expr_literal_int() {
    let value = mpack!({ "type": "literal", "value": 42 });
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
    let value = mpack!({ "type": "literal", "value": true });
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
    let value = mpack!({ "type": "literal", "value": null });
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
    let v = QueryValue::Null;
    assert!(matches!(
        expr_value_from_value(&v),
        Ok(SelectExprValue::Null)
    ));

    // Bool
    let v = QueryValue::Bool(true);
    assert!(matches!(
        expr_value_from_value(&v),
        Ok(SelectExprValue::Bool(true))
    ));

    // Int
    let v = QueryValue::Int(42);
    assert!(matches!(
        expr_value_from_value(&v),
        Ok(SelectExprValue::Int(42))
    ));

    // Float
    let v = QueryValue::F64(3.25);
    assert!(matches!(
        expr_value_from_value(&v),
        Ok(SelectExprValue::Float(_))
    ));

    // String
    let v = QueryValue::Str("hello".to_string());
    assert!(matches!(expr_value_from_value(&v), Ok(SelectExprValue::String(s)) if s == "hello"));
}

// ============================================================================
// ERROR Tests
// ============================================================================

#[test]
fn test_error_not_an_object() {
    let value = QueryValue::Str("not an object".to_string());
    let result = filter_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::InvalidType(_, _))));
}

#[test]
fn test_error_missing_field() {
    let value = mpack!({ "op": "eq" });
    let result = filter_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::MissingField(_))));
}

#[test]
fn test_error_unknown_filter_op() {
    let value = mpack!({ "op": "unknown", "field": "x", "value": 1 });
    let result = filter_from_value(&value);
    assert!(matches!(result, Err(QueryParseError::UnknownFilterOp(_))));
}

#[test]
fn test_error_unknown_agg_func() {
    assert!(agg_func_from_str("unknown_func").is_err());
}
