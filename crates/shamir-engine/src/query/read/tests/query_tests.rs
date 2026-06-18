//! Tests for Query parsing from QueryValue
//!
//! All tests construct QueryValue directly via mpack! and pass to the parser.

use crate::query::filter::{Filter, FilterValue};
use crate::query::read::query_from_value;
use crate::query::read::{AggFunc, ReadQuery, SelectItem};
use crate::query::TableRef;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

/// Construct a ReadQuery from a QueryValue produced by mpack!.
fn parse_query(value: QueryValue) -> ReadQuery {
    query_from_value(&value).expect("Failed to parse query")
}

#[test]
fn test_simple_select_all() {
    let query = parse_query(mpack!({
        "from": "users",
        "select": {
            "items": [
                { "type": "all" }
            ]
        }
    }));

    assert_eq!(query.from, TableRef::new("users"));
    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(query.select.items[0], SelectItem::All));
    assert!(query.r#where.is_none());
    assert!(query.group_by.is_none());
    assert!(query.order_by.is_none());
}

#[test]
fn test_select_fields() {
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
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Field { path, alias } if *path == vec!["name".to_string()] && alias.is_none()
    ));
    assert!(matches!(
        &query.select.items[1],
        SelectItem::Field { path, alias } if *path == vec!["email".to_string()] && alias.is_none()
    ));
    assert!(matches!(
        &query.select.items[2],
        SelectItem::Field { path, alias } if *path == vec!["age".to_string()] && alias.is_none()
    ));
}

#[test]
fn test_select_fields_with_aliases() {
    let query = parse_query(mpack!({
        "from": "users",
        "select": {
            "items": [
                { "type": "field", "path": "name", "alias": "user_name" },
                { "type": "field", "path": "email", "alias": "user_email" }
            ]
        }
    }));

    assert_eq!(query.select.items.len(), 2);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Field { path, alias } if *path == vec!["name".to_string()] && alias == &Some("user_name".to_string())
    ));
    assert!(matches!(
        &query.select.items[1],
        SelectItem::Field { path, alias } if *path == vec!["email".to_string()] && alias == &Some("user_email".to_string())
    ));
}

#[test]
fn test_select_with_where() {
    let query = parse_query(mpack!({
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
    }));

    assert!(query.r#where.is_some());
    let filter = query.r#where.unwrap();
    assert!(matches!(
        filter,
        Filter::Eq { field, value } if field == vec!["status".to_string()] && value == FilterValue::String("active".to_string())
    ));
}

#[test]
fn test_select_with_limit_offset() {
    let query = parse_query(mpack!({
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
    }));

    assert!(matches!(
        query.pagination,
        crate::query::read::Pagination::LimitOffset {
            limit: Some(10),
            offset: 20
        }
    ));
}

#[test]
fn test_select_with_order_by() {
    let query = parse_query(mpack!({
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
    }));

    assert!(query.order_by.is_some());
    let order = query.order_by.unwrap();
    assert_eq!(order.items.len(), 2);
}

#[test]
fn test_select_with_group_by() {
    let query = parse_query(mpack!({
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
    }));

    assert!(query.group_by.is_some());
    let group = query.group_by.unwrap();
    assert_eq!(group.fields, vec![vec!["customer_id".to_string()]]);
}

#[test]
fn test_select_with_group_by_having() {
    let query = parse_query(mpack!({
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
    }));

    assert!(query.group_by.is_some());
    let group = query.group_by.unwrap();
    assert!(group.having.is_some());
}

#[test]
fn test_count_all() {
    let query = parse_query(mpack!({
        "from": "users",
        "select": {
            "items": [
                { "type": "count_all", "alias": "total_users" }
            ]
        }
    }));

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::CountAll { alias } if alias == &Some("total_users".to_string())
    ));
}

#[test]
fn test_aggregate_count() {
    let query = parse_query(mpack!({
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
    }));

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { func, alias, .. }
            if *func == AggFunc::Count && alias == &Some("emails_count".to_string())
    ));
}

#[test]
fn test_aggregate_sum() {
    let query = parse_query(mpack!({
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
    }));

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { func, alias, .. }
            if *func == AggFunc::Sum && alias == &Some("total_sales".to_string())
    ));
}

#[test]
fn test_aggregate_avg() {
    let query = parse_query(mpack!({
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
    }));

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { func, alias, .. }
            if *func == AggFunc::Avg && alias == &Some("avg_salary".to_string())
    ));
}

#[test]
fn test_aggregate_min_max() {
    let query = parse_query(mpack!({
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
    }));

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
    let query = parse_query(mpack!({
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
    }));

    assert_eq!(query.select.items.len(), 1);
    assert!(matches!(
        &query.select.items[0],
        SelectItem::Aggregate { distinct, .. } if *distinct
    ));
}

#[test]
fn test_complex_query() {
    let query = parse_query(mpack!({
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
    }));

    assert_eq!(query.from, TableRef::new("orders"));
    assert_eq!(query.select.items.len(), 4);
    assert!(query.r#where.is_some());
    assert!(query.group_by.is_some());
    assert!(query.order_by.is_some());
    assert!(matches!(
        query.pagination,
        crate::query::read::Pagination::LimitOffset {
            limit: Some(100),
            offset: 0
        }
    ));
}

#[test]
fn test_sales_report() {
    let query = parse_query(mpack!({
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
    }));

    assert_eq!(query.from, TableRef::new("orders"));
    assert_eq!(query.select.items.len(), 5);
    assert!(query.r#where.is_some());
    assert!(query.group_by.is_some());
    let group = query.group_by.unwrap();
    assert!(group.having.is_some());
}
