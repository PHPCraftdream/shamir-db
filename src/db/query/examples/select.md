# SELECT Query Examples

This document contains JSON examples for SELECT queries.

## Table of Contents

- [Simple SELECT *](#simple-select-)
- [SELECT Specific Fields](#select-specific-fields)
- [SELECT with WHERE](#select-with-where)
- [SELECT with ORDER BY](#select-with-order-by)
- [SELECT with LIMIT/OFFSET](#select-with-limitoffset)
- [SELECT with GROUP BY](#select-with-group-by)
- [Complex SELECT](#complex-select)

---

## Simple SELECT *

Select all fields from a table.

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "all" }
        ]
    }
}
```

## SELECT Specific Fields

Select specific fields with optional aliases.

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "field", "path": "name" },
            { "type": "field", "path": "email" },
            { "type": "field", "path": "age" }
        ]
    }
}
```

With aliases:

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "field", "path": "name", "alias": "user_name" },
            { "type": "field", "path": "email", "alias": "user_email" }
        ]
    }
}
```

## SELECT with WHERE

Simple equality filter:

```json
{
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
}
```

Multiple conditions with AND:

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "all" }
        ]
    },
    "where": {
        "op": "and",
        "filters": [
            { "op": "eq", "field": "status", "value": "active" },
            { "op": "gt", "field": "age", "value": 18 }
        ]
    }
}
```

## SELECT with ORDER BY

Single field ascending:

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "all" }
        ]
    },
    "order_by": {
        "items": [
            { "field": "name", "order": "asc" }
        ]
    }
}
```

Multiple fields with NULL handling:

```json
{
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
}
```

## SELECT with LIMIT/OFFSET

Simple limit:

```json
{
    "from": "users",
    "select": {
        "items": [
            { "type": "all" }
        ]
    },
    "limit": {
        "limit": 10
    }
}
```

With offset (pagination):

```json
{
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
}
```

## SELECT with GROUP BY

Simple grouping:

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
    }
}
```

With HAVING clause:

```json
{
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
}
```

## Complex SELECT

Full-featured query with all clauses:

```json
{
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
}
```
