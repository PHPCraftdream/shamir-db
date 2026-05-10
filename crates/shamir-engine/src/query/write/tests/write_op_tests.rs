//! Tests for write operations using JSON format.

use serde_json::json;

use crate::query::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use crate::query::TableRef;

fn parse_insert(json: serde_json::Value) -> InsertOp {
    serde_json::from_value(json).expect("Failed to parse InsertOp")
}

fn parse_update(json: serde_json::Value) -> UpdateOp {
    serde_json::from_value(json).expect("Failed to parse UpdateOp")
}

fn parse_set(json: serde_json::Value) -> SetOp {
    serde_json::from_value(json).expect("Failed to parse SetOp")
}

fn parse_delete(json: serde_json::Value) -> DeleteOp {
    serde_json::from_value(json).expect("Failed to parse DeleteOp")
}

// ============================================================================
// INSERT TESTS
// ============================================================================

#[test]
fn test_insert_single_record() {
    let json = json!({
        "insert_into": "users",
        "values": [
            {
                "name": "Alice",
                "email": "alice@example.com"
            }
        ]
    });

    let op = parse_insert(json);

    assert_eq!(op.insert_into, TableRef::new("users"));
    assert_eq!(op.values.len(), 1);
    assert_eq!(op.values[0]["name"], "Alice");
    assert_eq!(op.values[0]["email"], "alice@example.com");
}

#[test]
fn test_insert_multiple_records() {
    let json = json!({
        "insert_into": "users",
        "values": [
            { "name": "Alice", "email": "alice@example.com" },
            { "name": "Bob", "email": "bob@example.com" },
            { "name": "Charlie", "email": "charlie@example.com" }
        ]
    });

    let op = parse_insert(json);

    assert_eq!(op.insert_into, TableRef::new("users"));
    assert_eq!(op.values.len(), 3);
    assert_eq!(op.values[0]["name"], "Alice");
    assert_eq!(op.values[1]["name"], "Bob");
    assert_eq!(op.values[2]["name"], "Charlie");
}

#[test]
fn test_insert_nested_data() {
    let json = json!({
        "insert_into": "orders",
        "values": [
            {
                "id": 1,
                "user_id": 100,
                "items": [
                    { "product_id": 1, "qty": 2 },
                    { "product_id": 3, "qty": 1 }
                ],
                "metadata": {
                    "source": "web",
                    "coupon": "SAVE10"
                }
            }
        ]
    });

    let op = parse_insert(json);

    assert_eq!(op.insert_into, TableRef::new("orders"));
    assert_eq!(op.values[0]["id"], 1);
    assert_eq!(op.values[0]["items"].as_array().unwrap().len(), 2);
    assert_eq!(op.values[0]["metadata"]["source"], "web");
}

#[test]
fn test_insert_roundtrip() {
    let json = json!({
        "insert_into": "products",
        "values": [
            { "id": 1, "name": "Widget", "price": 9.99 }
        ]
    });

    let op: InsertOp = serde_json::from_value(json.clone()).unwrap();
    let serialized = serde_json::to_value(&op).unwrap();

    assert_eq!(json, serialized);
}

// ============================================================================
// UPDATE TESTS
// ============================================================================

#[test]
fn test_update_with_filter() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "name": "New Name",
            "status": "active"
        }
    });

    let op = parse_update(json);

    assert_eq!(op.update, TableRef::new("users"));
    assert!(op.where_clause.is_some());
    assert_eq!(op.set["name"], "New Name");
    assert_eq!(op.set["status"], "active");
}

#[test]
fn test_update_without_filter() {
    let json = json!({
        "update": "products",
        "set": {
            "status": "discontinued"
        }
    });

    let op = parse_update(json);

    assert_eq!(op.update, TableRef::new("products"));
    assert!(op.where_clause.is_none());
    assert_eq!(op.set["status"], "discontinued");
}

#[test]
fn test_update_with_complex_filter() {
    let json = json!({
        "update": "orders",
        "where": {
            "op": "and",
            "filters": [
                { "op": "eq", "field": ["status"], "value": "pending" },
                { "op": "lt", "field": ["created_at"], "value": "2024-01-01" }
            ]
        },
        "set": {
            "status": "expired"
        }
    });

    let op = parse_update(json);

    assert_eq!(op.update, TableRef::new("orders"));
    assert!(op.where_clause.is_some());
    assert_eq!(op.set["status"], "expired");
}

#[test]
fn test_update_full_record() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "id": 1,
            "name": "Full",
            "email": "full@example.com",
            "status": "active",
            "created_at": "2024-01-15T10:30:00Z"
        }
    });

    let op = parse_update(json);

    assert_eq!(op.update, TableRef::new("users"));
    assert_eq!(op.set["id"], 1);
    assert_eq!(op.set["name"], "Full");
    assert_eq!(op.set["email"], "full@example.com");
}

#[test]
fn test_update_roundtrip() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "name": "Updated"
        }
    });

    let op: UpdateOp = serde_json::from_value(json.clone()).unwrap();
    let serialized = serde_json::to_value(&op).unwrap();

    assert_eq!(json, serialized);
}

#[test]
fn test_update_serializes_without_optional_where() {
    let json = json!({
        "update": "users",
        "set": {
            "status": "active"
        }
    });

    let op: UpdateOp = serde_json::from_value(json).unwrap();
    let serialized = serde_json::to_string(&op).unwrap();

    assert!(!serialized.contains("where"));
    assert!(serialized.contains("update"));
    assert!(serialized.contains("set"));
}

// ============================================================================
// UPDATE SELECT TESTS
// ============================================================================

#[test]
fn test_update_select_changed_mode() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["status"],
            "value": "inactive"
        },
        "set": {
            "status": "active"
        },
        "select": {
            "return_mode": "changed"
        }
    });

    let op = parse_update(json);

    assert_eq!(op.update, TableRef::new("users"));
    assert!(op.select.is_some());
    let select = op.select.unwrap();
    assert_eq!(
        select.return_mode,
        crate::query::write::UpdateReturnMode::Changed
    );
    assert!(select.fields.is_none());
}

#[test]
fn test_update_select_all_mode() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "name": "Updated"
        },
        "select": {
            "return_mode": "all"
        }
    });

    let op = parse_update(json);

    let select = op.select.unwrap();
    assert_eq!(
        select.return_mode,
        crate::query::write::UpdateReturnMode::All
    );
}

#[test]
fn test_update_select_unchanged_mode() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "status": "active"
        },
        "select": {
            "return_mode": "unchanged"
        }
    });

    let op = parse_update(json);

    let select = op.select.unwrap();
    assert_eq!(
        select.return_mode,
        crate::query::write::UpdateReturnMode::Unchanged
    );
}

#[test]
fn test_update_select_with_fields() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "name": "Updated",
            "status": "active"
        },
        "select": {
            "return_mode": "changed",
            "fields": ["id", "name", "status"]
        }
    });

    let op = parse_update(json);

    let select = op.select.unwrap();
    assert_eq!(
        select.return_mode,
        crate::query::write::UpdateReturnMode::Changed
    );
    assert_eq!(
        select.fields,
        Some(vec![
            "id".to_string(),
            "name".to_string(),
            "status".to_string()
        ])
    );
}

#[test]
fn test_update_select_roundtrip() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "name": "Updated"
        },
        "select": {
            "return_mode": "changed",
            "fields": ["id", "name"]
        }
    });

    let op: UpdateOp = serde_json::from_value(json.clone()).unwrap();
    let serialized = serde_json::to_value(&op).unwrap();

    assert_eq!(json, serialized);
}

#[test]
fn test_update_without_select() {
    let json = json!({
        "update": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        },
        "set": {
            "name": "Updated"
        }
    });

    let op = parse_update(json);

    assert!(op.select.is_none());
}

#[test]
fn test_update_select_serializes_without_optional_fields() {
    let json = json!({
        "update": "users",
        "set": {
            "status": "active"
        },
        "select": {
            "return_mode": "changed"
        }
    });

    let op: UpdateOp = serde_json::from_value(json).unwrap();
    let serialized = serde_json::to_string(&op).unwrap();

    assert!(serialized.contains("select"));
    assert!(serialized.contains("changed"));
    assert!(!serialized.contains("fields"));
}

#[test]
fn test_update_select_default_mode() {
    let json = json!({
        "update": "users",
        "set": {
            "status": "active"
        },
        "select": {}
    });

    let op = parse_update(json);

    let select = op.select.unwrap();
    assert_eq!(
        select.return_mode,
        crate::query::write::UpdateReturnMode::Changed
    );
}

// ============================================================================
// SET (UPSERT) TESTS
// ============================================================================

#[test]
fn test_set_by_primary_key() {
    let json = json!({
        "set": "users",
        "key": {
            "id": 1
        },
        "value": {
            "name": "Alice",
            "email": "alice@example.com"
        }
    });

    let op = parse_set(json);

    assert_eq!(op.set, TableRef::new("users"));
    assert_eq!(op.key["id"], 1);
    assert_eq!(op.value["name"], "Alice");
    assert_eq!(op.value["email"], "alice@example.com");
}

#[test]
fn test_set_by_unique_field() {
    let json = json!({
        "set": "users",
        "key": {
            "email": "alice@example.com"
        },
        "value": {
            "name": "Alice Updated"
        }
    });

    let op = parse_set(json);

    assert_eq!(op.set, TableRef::new("users"));
    assert_eq!(op.key["email"], "alice@example.com");
    assert_eq!(op.value["name"], "Alice Updated");
}

#[test]
fn test_set_composite_key() {
    let json = json!({
        "set": "order_items",
        "key": {
            "order_id": 1,
            "product_id": 5
        },
        "value": {
            "qty": 3,
            "price": 19.99
        }
    });

    let op = parse_set(json);

    assert_eq!(op.set, TableRef::new("order_items"));
    assert_eq!(op.key["order_id"], 1);
    assert_eq!(op.key["product_id"], 5);
    assert_eq!(op.value["qty"], 3);
}

#[test]
fn test_set_roundtrip() {
    let json = json!({
        "set": "users",
        "key": {
            "id": 1
        },
        "value": {
            "name": "Alice"
        }
    });

    let op: SetOp = serde_json::from_value(json.clone()).unwrap();
    let serialized = serde_json::to_value(&op).unwrap();

    assert_eq!(json, serialized);
}

// ============================================================================
// DELETE TESTS
// ============================================================================

#[test]
fn test_delete_with_filter() {
    let json = json!({
        "delete_from": "users",
        "where": {
            "op": "eq",
            "field": ["status"],
            "value": "inactive"
        }
    });

    let op = parse_delete(json);

    assert_eq!(op.delete_from, TableRef::new("users"));
}

#[test]
fn test_delete_with_complex_filter() {
    let json = json!({
        "delete_from": "logs",
        "where": {
            "op": "and",
            "filters": [
                { "op": "lt", "field": ["created_at"], "value": "2023-01-01" },
                { "op": "eq", "field": ["archived"], "value": true }
            ]
        }
    });

    let op = parse_delete(json);

    assert_eq!(op.delete_from, TableRef::new("logs"));
}

#[test]
fn test_delete_by_id() {
    let json = json!({
        "delete_from": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 42
        }
    });

    let op = parse_delete(json);

    assert_eq!(op.delete_from, TableRef::new("users"));
}

#[test]
fn test_delete_roundtrip() {
    let json = json!({
        "delete_from": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1
        }
    });

    let op: DeleteOp = serde_json::from_value(json.clone()).unwrap();
    let serialized = serde_json::to_value(&op).unwrap();

    assert_eq!(json, serialized);
}

// ============================================================================
// ERROR CASES
// ============================================================================

#[test]
fn test_insert_requires_values() {
    let json = json!({
        "insert_into": "users"
    });

    let result: Result<InsertOp, _> = serde_json::from_value(json);

    assert!(result.is_err());
}

#[test]
fn test_delete_requires_where() {
    let json = json!({
        "delete_from": "users"
    });

    let result: Result<DeleteOp, _> = serde_json::from_value(json);

    assert!(result.is_err());
}

#[test]
fn test_set_requires_key() {
    let json = json!({
        "set": "users",
        "value": {
            "name": "Alice"
        }
    });

    let result: Result<SetOp, _> = serde_json::from_value(json);

    assert!(result.is_err());
}

#[test]
fn test_set_requires_value() {
    let json = json!({
        "set": "users",
        "key": {
            "id": 1
        }
    });

    let result: Result<SetOp, _> = serde_json::from_value(json);

    assert!(result.is_err());
}

// ============================================================================
// SPECIAL TYPES
// ============================================================================

#[test]
fn test_insert_with_null() {
    let json = json!({
        "insert_into": "users",
        "values": [
            {
                "name": "Alice",
                "email": null
            }
        ]
    });

    let op = parse_insert(json);

    assert!(op.values[0]["email"].is_null());
}

#[test]
fn test_insert_with_special_characters() {
    let json = json!({
        "insert_into": "users",
        "values": [
            {
                "name": "O'Brien",
                "bio": "Line1\nLine2\tTabbed",
                "emoji": "😀🎉"
            }
        ]
    });

    let op = parse_insert(json);

    assert_eq!(op.values[0]["name"], "O'Brien");
    assert_eq!(op.values[0]["emoji"], "😀🎉");
}

#[test]
fn test_insert_with_numbers() {
    let json = json!({
        "insert_into": "products",
        "values": [
            {
                "id": 1,
                "price": 99.99,
                "stock": 100,
                "weight": 1.5e-3
            }
        ]
    });

    let op = parse_insert(json);

    assert_eq!(op.values[0]["id"], 1);
    assert_eq!(op.values[0]["price"], 99.99);
    assert_eq!(op.values[0]["stock"], 100);
}

#[test]
fn test_insert_with_boolean() {
    let json = json!({
        "insert_into": "flags",
        "values": [
            {
                "active": true,
                "deleted": false
            }
        ]
    });

    let op = parse_insert(json);

    assert_eq!(op.values[0]["active"], true);
    assert_eq!(op.values[0]["deleted"], false);
}
