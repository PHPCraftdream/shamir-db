//! Tests for Query parsing from JSON
//!
//! All tests use JSON strings as source, parse to QueryValue, then convert to Query.

use crate::db::query::filter::{Filter, FilterValue};
use crate::db::query::read::query_from_value;
use crate::db::query::read::{AggFunc, ReadQuery, SelectItem};
use crate::types::value::QueryValue;

/// Parse JSON string to QueryValue, then to Query
fn parse_query(json: &str) -> ReadQuery {
    let query_value: QueryValue = serde_json::from_str(json).expect("Invalid JSON");
    query_from_value(&query_value).expect("Failed to parse query")
}

#[test]
fn test_simple_select_all() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                { "type": "all" }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.from, "users");
    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(query.select.items[0], SelectItem::All));
    assert!(query.r#where.is_none());
    assert!(query.group_by.is_none());
    assert!(query.order_by.is_none());
}

#[test]
fn test_select_fields() {
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
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Field { path, alias } if path == "name" && alias.is_none()
    ));
    assert!(matches!(
        &query.select.items[1],
        SelectItem::Field { path, alias } if path == "email" && alias.is_none()
    ));
    assert!(matches!(
        &query.select.items[2],
        SelectItem::Field { path, alias } if path == "age" && alias.is_none()
    ));
}

#[test]
fn test_select_fields_with_aliases() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                { "type": "field", "path": "name", "alias": "user_name" },
                { "type": "field", "path": "email", "alias": "user_email" }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 2);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Field { path, alias } if path == "name" && alias == &Some("user_name".to_string())
    ));
    assert!(matches!(
        &query.select.items[1],
        SelectItem::Field { path, alias } if path == "email" && alias == &Some("user_email".to_string())
    ));
}

#[test]
fn test_select_with_where() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                { "type": "all" }
            ]
        },
        "where": {
            "op": "eq",
            "field": "status",
            "value": "active"
        }
    }"#;

    let query = parse_query(json);

    assert!(query.r#where.is_some());
    let filter = query.r#where.unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value } if field == "status" && value == FilterValue::String("active".to_string())
    ));
}

#[test]
fn test_select_with_limit_offset() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                { "type": "all" }
            ]
        },
        "limit": {
            "limit": 10,
            "offset": 20
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.limit.limit, Some(10));
    assert_eq!(query.limit.offset, 20);
}

#[test]
fn test_select_with_order_by() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                { "type": "all" }
            ]
        },
        "order_by": {
            "items": [
                { "field": "created_at", "order": "desc", "nulls": "last" },
                { "field": "name", "order": "asc" }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert!(query.order_by.is_some());
    let order = query.order_by.unwrap();
    assert_eq!(order.items.len(), 2);
}

#[test]
fn test_select_with_group_by() {
    let json = r#"{
        "from": "orders",
        "select": {
            "items": [
                { "type": "field", "path": "customer_id" },
                {
                    "type": "aggregate",
                    "func": "sum",
                    "field": { "type": "field", "name": "total" },
                    "alias": "total_spent"
                }
            ]
        },
        "group_by": {
            "fields": ["customer_id"]
        }
    }"#;

    let query = parse_query(json);

    assert!(query.group_by.is_some());
    let group = query.group_by.unwrap();
    assert_eq!(group.fields, vec!["customer_id"]);
}

#[test]
fn test_select_with_group_by_having() {
    let json = r#"{
        "from": "orders",
        "select": {
            "items": [
                { "type": "field", "path": "customer_id" },
                {
                    "type": "aggregate",
                    "func": "count",
                    "field": { "type": "field", "name": "id" },
                    "alias": "order_count"
                }
            ]
        },
        "group_by": {
            "fields": ["customer_id"],
            "having": {
                "op": "gt",
                "field": "order_count",
                "value": 5
            }
        }
    }"#;

    let query = parse_query(json);

    assert!(query.group_by.is_some());
    let group = query.group_by.unwrap();
    assert!(group.having.is_some());
}

#[test]
fn test_count_all() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                { "type": "count_all", "alias": "total_users" }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::CountAll { alias } if alias == &Some("total_users".to_string())
    ));
}

#[test]
fn test_aggregate_count() {
    let json = r#"{
        "from": "users",
        "select": {
            "items": [
                {
                    "type": "aggregate",
                    "func": "count",
                    "field": { "type": "field", "name": "email" },
                    "alias": "emails_count"
                }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { func, alias, .. }
            if *func == AggFunc::Count && alias == &Some("emails_count".to_string())
    ));
}

#[test]
fn test_aggregate_sum() {
    let json = r#"{
        "from": "orders",
        "select": {
            "items": [
                {
                    "type": "aggregate",
                    "func": "sum",
                    "field": { "type": "field", "name": "total" },
                    "alias": "total_sales"
                }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { func, alias, .. }
            if *func == AggFunc::Sum && alias == &Some("total_sales".to_string())
    ));
}

#[test]
fn test_aggregate_avg() {
    let json = r#"{
        "from": "employees",
        "select": {
            "items": [
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

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { func, alias, .. }
            if *func == AggFunc::Avg && alias == &Some("avg_salary".to_string())
    ));
}

#[test]
fn test_aggregate_min_max() {
    let json = r#"{
        "from": "products",
        "select": {
            "items": [
                {
                    "type": "aggregate",
                    "func": "min",
                    "field": { "type": "field", "name": "price" },
                    "alias": "min_price"
                },
                {
                    "type": "aggregate",
                    "func": "max",
                    "field": { "type": "field", "name": "price" },
                    "alias": "max_price"
                }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 2);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { func, .. } if *func == AggFunc::Min
    ));
    assert!(matches!(
        &query.select.items[1],
        SelectItem::Aggregate { func, .. } if *func == AggFunc::Max
    ));
}

#[test]
fn test_aggregate_distinct() {
    let json = r#"{
        "from": "orders",
        "select": {
            "items": [
                {
                    "type": "aggregate",
                    "func": "count",
                    "field": { "type": "field", "name": "customer_id" },
                    "alias": "unique_customers",
                    "distinct": true
                }
            ]
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { distinct, .. } if *distinct
    ));
}

#[test]
fn test_complex_query() {
    let json = r#"{
        "from": "orders",
        "select": {
            "items": [
                { "type": "field", "path": "customer_id" },
                { "type": "field", "path": "status" },
                {
                    "type": "aggregate",
                    "func": "sum",
                    "field": { "type": "field", "name": "total" },
                    "alias": "total_spent"
                },
                {
                    "type": "aggregate",
                    "func": "count",
                    "field": { "type": "field", "name": "id" },
                    "alias": "order_count"
                }
            ],
            "distinct": false
        },
        "where": {
            "op": "and",
            "filters": [
                { "op": "gte", "field": "created_at", "value": "2024-01-01" },
                { "op": "eq", "field": "status", "value": "completed" }
            ]
        },
        "group_by": {
            "fields": ["customer_id", "status"]
        },
        "order_by": {
            "items": [
                { "field": "total_spent", "order": "desc" }
            ]
        },
        "limit": {
            "limit": 100,
            "offset": 0
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.from, "orders");
    assert_eq!(query.select.items.len(), 4);
    assert!(query.r#where.is_some());
    assert!(query.group_by.is_some());
    assert!(query.order_by.is_some());
    assert_eq!(query.limit.limit, Some(100));
    assert_eq!(query.limit.offset, 0);
}

#[test]
fn test_sales_report() {
    let json = r#"{
        "from": "orders",
        "select": {
            "items": [
                { "type": "field", "path": "region" },
                { "type": "field", "path": "product_category" },
                {
                    "type": "aggregate",
                    "func": "count",
                    "field": { "type": "field", "name": "id" },
                    "alias": "orders"
                },
                {
                    "type": "aggregate",
                    "func": "sum",
                    "field": { "type": "field", "name": "total" },
                    "alias": "revenue"
                },
                {
                    "type": "aggregate",
                    "func": "avg",
                    "field": { "type": "field", "name": "total" },
                    "alias": "avg_order"
                }
            ]
        },
        "where": {
            "op": "gte",
            "field": "created_at",
            "value": "2024-01-01"
        },
        "group_by": {
            "fields": ["region", "product_category"],
            "having": {
                "op": "gt",
                "field": "revenue",
                "value": 10000
            }
        },
        "order_by": {
            "items": [
                { "field": "revenue", "order": "desc" }
            ]
        },
        "limit": {
            "limit": 50
        }
    }"#;

    let query = parse_query(json);

    assert_eq!(query.from, "orders");
    assert_eq!(query.select.items.len(), 5);
    assert!(query.r#where.is_some());
    assert!(query.group_by.is_some());
    let group = query.group_by.unwrap();
    assert!(group.having.is_some());
}
