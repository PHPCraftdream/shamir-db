# Filter (WHERE Clause) Examples

This document contains JSON examples for WHERE filters.

## Table of Contents

- [Comparison Operators](#comparison-operators)
- [Logical Operators](#logical-operators)
- [NULL Checks](#null-checks)
- [Array Operations](#array-operations)
- [Nested Conditions](#nested-conditions)

---

## Comparison Operators

### Equality (`eq`)

```json
{
    "op": "eq",
    "field": "status",
    "value": "active"
}
```

### Inequality (`ne`)

```json
{
    "op": "ne",
    "field": "status",
    "value": "deleted"
}
```

### Greater Than (`gt`)

```json
{
    "op": "gt",
    "field": "age",
    "value": 18
}
```

### Greater Than or Equal (`gte`)

```json
{
    "op": "gte",
    "field": "salary",
    "value": 50000
}
```

### Less Than (`lt`)

```json
{
    "op": "lt",
    "field": "age",
    "value": 65
}
```

### Less Than or Equal (`lte`)

```json
{
    "op": "lte",
    "field": "stock",
    "value": 100
}
```

### Value Types

String:
```json
{
    "op": "eq",
    "field": "name",
    "value": "John"
}
```

Integer:
```json
{
    "op": "eq",
    "field": "count",
    "value": 42
}
```

Float:
```json
{
    "op": "eq",
    "field": "price",
    "value": 19.99
}
```

Boolean:
```json
{
    "op": "eq",
    "field": "active",
    "value": true
}
```

Null:
```json
{
    "op": "eq",
    "field": "deleted_at",
    "value": null
}
```

## Logical Operators

### AND

```json
{
    "op": "and",
    "filters": [
        { "op": "eq", "field": "status", "value": "active" },
        { "op": "gt", "field": "age", "value": 18 }
    ]
}
```

### OR

```json
{
    "op": "or",
    "filters": [
        { "op": "eq", "field": "role", "value": "admin" },
        { "op": "eq", "field": "role", "value": "moderator" }
    ]
}
```

### NOT

```json
{
    "op": "not",
    "filter": {
        "op": "eq",
        "field": "status",
        "value": "deleted"
    }
}
```

## NULL Checks

### IS NULL

```json
{
    "op": "is_null",
    "field": "deleted_at"
}
```

### IS NOT NULL

```json
{
    "op": "is_not_null",
    "field": "email_verified_at"
}
```

## Array Operations

### IN (value in array)

```json
{
    "op": "in",
    "field": "status",
    "values": ["active", "pending", "review"]
}
```

### NOT IN

```json
{
    "op": "not_in",
    "field": "role",
    "values": ["banned", "deleted"]
}
```

### BETWEEN

```json
{
    "op": "between",
    "field": "age",
    "from": 18,
    "to": 65
}
```

## Nested Conditions

### Two-Level Nesting

Active users who are either admins OR (moderators with verification):

```json
{
    "op": "and",
    "filters": [
        { "op": "eq", "field": "status", "value": "active" },
        {
            "op": "or",
            "filters": [
                { "op": "eq", "field": "role", "value": "admin" },
                {
                    "op": "and",
                    "filters": [
                        { "op": "eq", "field": "role", "value": "moderator" },
                        { "op": "eq", "field": "verified", "value": true }
                    ]
                }
            ]
        }
    ]
}
```

### Three-Level Nesting

Complex permission check:

```json
{
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
}
```

### NOT with Complex Condition

Not (banned OR deleted):

```json
{
    "op": "not",
    "filter": {
        "op": "or",
        "filters": [
            { "op": "eq", "field": "status", "value": "banned" },
            { "op": "eq", "field": "status", "value": "deleted" }
        ]
    }
}
```
