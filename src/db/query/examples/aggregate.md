# Aggregation Examples

This document contains JSON examples for aggregation queries.

## Table of Contents

- [COUNT](#count)
- [SUM](#sum)
- [AVG](#avg)
- [MIN](#min)
- [MAX](#max)
- [Combined Aggregations](#combined-aggregations)
- [DISTINCT Aggregations](#distinct-aggregations)

---

## COUNT

### COUNT(*)

Count all rows:

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "count_all", "alias": "total_users" }
        ]
    }
}
```

### COUNT(field)

Count non-null values in a field:

```json
{
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
}
```

### COUNT with GROUP BY

Count users per department:

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "field", "path": "department" },
            {
                "type": "aggregate",
                "func": "count",
                "field": { "type": "field", "name": "id" },
                "alias": "count"
            }
        ]
    },
    "group_by": {
        "fields": ["department"]
    }
}
```

## SUM

### Simple SUM

Total sales:

```json
{
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
}
```

### SUM with GROUP BY

Revenue per customer:

```json
{
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
    },
    "order_by": {
        "items": [
            { "field": "total_spent", "order": "desc" }
        ]
    }
}
```

## AVG

### Simple Average

Average salary:

```json
{
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
}
```

### AVG with GROUP BY

Average salary per department:

```json
{
    "from": "employees",
    "select": {
        "items": [
            { "type": "field", "path": "department" },
            {
                "type": "aggregate",
                "func": "avg",
                "field": { "type": "field", "name": "salary" },
                "alias": "avg_salary"
            }
        ]
    },
    "group_by": {
        "fields": ["department"]
    }
}
```

## MIN

### Minimum Value

Lowest price:

```json
{
    "from": "products",
    "select": {
        "items": [
            {
                "type": "aggregate",
                "func": "min",
                "field": { "type": "field", "name": "price" },
                "alias": "min_price"
            }
        ]
    }
}
```

## MAX

### Maximum Value

Highest salary:

```json
{
    "from": "employees",
    "select": {
        "items": [
            {
                "type": "aggregate",
                "func": "max",
                "field": { "type": "field", "name": "salary" },
                "alias": "max_salary"
            }
        ]
    }
}
```

## Combined Aggregations

Multiple aggregations in one query:

```json
{
    "from": "orders",
    "select": {
        "items": [
            { "type": "field", "path": "status" },
            {
                "type": "aggregate",
                "func": "count",
                "field": { "type": "field", "name": "id" },
                "alias": "order_count"
            },
            {
                "type": "aggregate",
                "func": "sum",
                "field": { "type": "field", "name": "total" },
                "alias": "total_revenue"
            },
            {
                "type": "aggregate",
                "func": "avg",
                "field": { "type": "field", "name": "total" },
                "alias": "avg_order_value"
            },
            {
                "type": "aggregate",
                "func": "min",
                "field": { "type": "field", "name": "total" },
                "alias": "min_order"
            },
            {
                "type": "aggregate",
                "func": "max",
                "field": { "type": "field", "name": "total" },
                "alias": "max_order"
            }
        ]
    },
    "group_by": {
        "fields": ["status"]
    }
}
```

## DISTINCT Aggregations

### COUNT DISTINCT

Count unique values:

```json
{
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
}
```

### SUM DISTINCT

Sum unique values only:

```json
{
    "from": "sales",
    "select": {
        "items": [
            {
                "type": "aggregate",
                "func": "sum",
                "field": { "type": "field", "name": "amount" },
                "alias": "unique_amount",
                "distinct": true
            }
        ]
    }
}
```

## Complete Examples

### Sales Report

```json
{
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
}
```
