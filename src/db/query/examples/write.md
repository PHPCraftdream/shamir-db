# Write Operations Examples

This document contains JSON examples for write operations: Insert, Update, Set, Delete.

## Table of Contents

- [Insert Operations](#insert-operations)
- [Update Operations](#update-operations)
- [Update with Select (Returning)](#update-with-select-returning)
- [Set Operations (Upsert)](#set-operations-upsert)
- [Delete Operations](#delete-operations)

---

## Insert Operations

### Single Record Insert

```json
{
  "insert_into": "users",
  "values": [
    { "name": "Alice", "email": "alice@example.com" }
  ]
}
```

### Multiple Records Insert

```json
{
  "insert_into": "products",
  "values": [
    { "name": "Product A", "price": 100, "category": "electronics" },
    { "name": "Product B", "price": 200, "category": "electronics" },
    { "name": "Product C", "price": 300, "category": "clothing" }
  ]
}
```

### Insert with Nested Data

```json
{
  "insert_into": "users",
  "values": [{
    "name": "Alice",
    "email": "alice@example.com",
    "profile": {
      "age": 30,
      "city": "Moscow",
      "interests": ["rust", "databases"]
    },
    "settings": {
      "notifications": true,
      "theme": "dark"
    }
  }]
}
```

### Insert with Arrays

```json
{
  "insert_into": "articles",
  "values": [{
    "title": "Introduction to Rust",
    "tags": ["rust", "programming", "tutorial"],
    "author_id": 1,
    "published": true
  }]
}
```

### Insert with NULL Values

```json
{
  "insert_into": "users",
  "values": [{
    "name": "Bob",
    "email": null,
    "deleted_at": null
  }]
}
```

---

## Update Operations

### Simple Update

```json
{
  "update": "users",
  "where": { "op": "eq", "field": "id", "value": 1 },
  "set": { "status": "active" }
}
```

### Update Multiple Fields

```json
{
  "update": "products",
  "where": { "op": "eq", "field": "category", "value": "electronics" },
  "set": {
    "discount": 0.1,
    "on_sale": true,
    "updated_at": "2024-01-15"
  }
}
```

### Update with Complex Filter

```json
{
  "update": "orders",
  "where": {
    "op": "and",
    "filters": [
      { "op": "eq", "field": "status", "value": "pending" },
      { "op": "lt", "field": "created_at", "value": "2024-01-01" }
    ]
  },
  "set": { "status": "expired" }
}
```

### Update All Records (No Filter)

```json
{
  "update": "products",
  "set": { "currency": "USD" }
}
```

### Update with IN Filter

```json
{
  "update": "users",
  "where": {
    "op": "in",
    "field": "role",
    "values": ["moderator", "editor"]
  },
  "set": { "permissions": "standard" }
}
```

---

## Update with Select (Returning)

### Return All Matched Records

```json
{
  "update": "orders",
  "where": { "op": "eq", "field": "processed", "value": false },
  "set": { "processed": true },
  "select": {
    "return_mode": "all"
  }
}
```

### Return Only Changed Records (Default)

```json
{
  "update": "users",
  "where": { "op": "gte", "field": "login_count", "value": 100 },
  "set": { "is_vip": true },
  "select": {
    "return_mode": "changed",
    "fields": ["id", "name", "is_vip"]
  }
}
```

### Return Unchanged Records

```json
{
  "update": "products",
  "where": { "op": "eq", "field": "category", "value": "electronics" },
  "set": { "category": "electronics" },
  "select": {
    "return_mode": "unchanged",
    "fields": ["id", "name"]
  }
}
```

### Update with Select Specific Fields

```json
{
  "update": "users",
  "where": { "op": "eq", "field": "status", "value": "inactive" },
  "set": { "status": "active" },
  "select": {
    "return_mode": "changed",
    "fields": ["id", "name", "status", "updated_at"]
  }
}
```

---

## Set Operations (Upsert)

### Set by Primary Key

```json
{
  "set": "users",
  "key": { "id": 1 },
  "value": { "name": "Alice Updated", "email": "alice@new.com" }
}
```

### Set by Unique Field (Email)

```json
{
  "set": "users",
  "key": { "email": "alice@example.com" },
  "value": { "name": "Alice", "status": "active" }
}
```

### Set with Partial Update

```json
{
  "set": "users",
  "key": { "id": 1 },
  "value": { "last_login": "2024-01-15T10:30:00Z" }
}
```

### Set with Composite Key

```json
{
  "set": "order_items",
  "key": { "order_id": 100, "product_id": 500 },
  "value": { "quantity": 5, "price": 99.99 }
}
```

### Set with Nested Data

```json
{
  "set": "users",
  "key": { "id": 1 },
  "value": {
    "name": "Alice",
    "profile": {
      "age": 31,
      "city": "Saint Petersburg"
    }
  }
}
```

---

## Delete Operations

### Delete by ID

```json
{
  "delete_from": "users",
  "where": { "op": "eq", "field": "id", "value": 123 }
}
```

### Delete with Simple Filter

```json
{
  "delete_from": "sessions",
  "where": { "op": "lt", "field": "expires_at", "value": "2024-01-01" }
}
```

### Delete with Complex Filter

```json
{
  "delete_from": "logs",
  "where": {
    "op": "and",
    "filters": [
      { "op": "lt", "field": "created_at", "value": "2023-01-01" },
      { "op": "eq", "field": "archived", "value": true }
    ]
  }
}
```

### Delete with IN Filter

```json
{
  "delete_from": "products",
  "where": {
    "op": "in",
    "field": "category_id",
    "values": [10, 20, 30]
  }
}
```

### Delete with OR Filter

```json
{
  "delete_from": "notifications",
  "where": {
    "op": "or",
    "filters": [
      { "op": "eq", "field": "read", "value": true },
      { "op": "lt", "field": "expires_at", "value": "2024-01-01" }
    ]
  }
}
```

---

## Using in Batch API

### Combined Read and Write

```json
{
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": "id", "value": 1 }
    },
    "update_orders": {
      "update": "orders",
      "where": { "op": "eq", "field": "user_id", "value": { "$query": "user[0].id" } },
      "set": { "status": "processed" }
    }
  }
}
```

### Full Workflow Example

```json
{
  "queries": {
    "inactive_users": {
      "from": "users",
      "where": {
        "op": "and",
        "filters": [
          { "op": "eq", "field": "status", "value": "inactive" },
          { "op": "lt", "field": "last_login", "value": "2023-01-01" }
        ]
      }
    },
    "deactivate": {
      "update": "users",
      "where": {
        "op": "in",
        "field": "id",
        "values": [{ "$query": "inactive_users[].id" }]
      },
      "set": { "status": "deleted" },
      "select": {
        "return_mode": "changed",
        "fields": ["id", "name", "status"]
      }
    },
    "cleanup_sessions": {
      "delete_from": "sessions",
      "where": {
        "op": "in",
        "field": "user_id",
        "values": [{ "$query": "deactivate[].id" }]
      }
    }
  }
}
```

---

## See Also

- [Batch Query System](../batch/README.md) — Complete Batch API documentation
- [Filter Examples](./filter.md) — WHERE clause examples
- [Select Examples](./select.md) — SELECT query examples
- [Write Operations](../write/README.md) — Write operations module documentation
