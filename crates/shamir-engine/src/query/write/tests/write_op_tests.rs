//! Tests for write operations — construction & parsing.
//!
//! Request-building uses the typed query builder (`shamir_query_builder`).
//! Roundtrip / error-case tests that specifically validate serde
//! deserialization still use `json!` — that IS the thing under test.

use serde_json::json;
use shamir_types::mpack;

use shamir_query_builder::filter;
use shamir_query_builder::filter::FilterExt;
use shamir_query_builder::write::{self, doc, UpdateReturnMode};

use crate::query::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use crate::query::TableRef;

// ============================================================================
// INSERT TESTS
// ============================================================================

#[test]
fn test_insert_single_record() {
    let op = write::insert("users")
        .row(doc().set("name", "Alice").set("email", "alice@example.com"))
        .build();

    assert_eq!(op.insert_into, TableRef::new("users"));
    assert_eq!(op.values.len(), 1);
    assert_eq!(op.values[0]["name"], "Alice");
    assert_eq!(op.values[0]["email"], "alice@example.com");
}

#[test]
fn test_insert_multiple_records() {
    let op = write::insert("users")
        .row(doc().set("name", "Alice").set("email", "alice@example.com"))
        .row(doc().set("name", "Bob").set("email", "bob@example.com"))
        .row(
            doc()
                .set("name", "Charlie")
                .set("email", "charlie@example.com"),
        )
        .build();

    assert_eq!(op.insert_into, TableRef::new("users"));
    assert_eq!(op.values.len(), 3);
    assert_eq!(op.values[0]["name"], "Alice");
    assert_eq!(op.values[1]["name"], "Bob");
    assert_eq!(op.values[2]["name"], "Charlie");
}

#[test]
fn test_insert_nested_data() {
    // Nested arrays/objects are supplied as QueryValue via set_value + mpack!.
    let op = write::insert("orders")
        .row(
            doc()
                .set("id", 1_i64)
                .set("user_id", 100_i64)
                .set_value(
                    "items",
                    mpack!([
                        { "product_id": 1, "qty": 2 },
                        { "product_id": 3, "qty": 1 }
                    ]),
                )
                .set_value(
                    "metadata",
                    mpack!({"source": "web", "coupon": "SAVE10"}),
                ),
        )
        .build();

    assert_eq!(op.insert_into, TableRef::new("orders"));
    assert_eq!(op.values[0]["id"], 1);
    assert_eq!(op.values[0]["items"].as_array().unwrap().len(), 2);
    assert_eq!(op.values[0]["metadata"]["source"], "web");
}

#[test]
fn test_insert_roundtrip() {
    // Roundtrip tests validate serde — keep json! for the expected wire form.
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
    let op = write::update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("name", "New Name").set("status", "active"))
        .build();

    assert_eq!(op.update, TableRef::new("users"));
    assert!(op.where_clause.is_some());
    assert_eq!(op.set["name"], "New Name");
    assert_eq!(op.set["status"], "active");
}

#[test]
fn test_update_without_filter() {
    let op = write::update("products")
        .set(doc().set("status", "discontinued"))
        .build();

    assert_eq!(op.update, TableRef::new("products"));
    assert!(op.where_clause.is_none());
    assert_eq!(op.set["status"], "discontinued");
}

#[test]
fn test_update_with_complex_filter() {
    let op = write::update("orders")
        .where_(filter::eq("status", "pending").and(filter::lt("created_at", "2024-01-01")))
        .set(doc().set("status", "expired"))
        .build();

    assert_eq!(op.update, TableRef::new("orders"));
    assert!(op.where_clause.is_some());
    assert_eq!(op.set["status"], "expired");
}

#[test]
fn test_update_full_record() {
    let op = write::update("users")
        .where_(filter::eq("id", 1_i64))
        .set(
            doc()
                .set("id", 1_i64)
                .set("name", "Full")
                .set("email", "full@example.com")
                .set("status", "active")
                .set("created_at", "2024-01-15T10:30:00Z"),
        )
        .build();

    assert_eq!(op.update, TableRef::new("users"));
    assert_eq!(op.set["id"], 1);
    assert_eq!(op.set["name"], "Full");
    assert_eq!(op.set["email"], "full@example.com");
}

#[test]
fn test_update_roundtrip() {
    // Roundtrip tests validate serde — keep json! for the expected wire form.
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
    let op = write::update("users")
        .set(doc().set("status", "active"))
        .build();
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
    let op = write::update("users")
        .where_(filter::eq("status", "inactive"))
        .set(doc().set("status", "active"))
        .returning(UpdateReturnMode::Changed)
        .build();

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
    let op = write::update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("name", "Updated"))
        .returning(UpdateReturnMode::All)
        .build();

    let select = op.select.unwrap();
    assert_eq!(
        select.return_mode,
        crate::query::write::UpdateReturnMode::All
    );
}

#[test]
fn test_update_select_unchanged_mode() {
    let op = write::update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("status", "active"))
        .returning(UpdateReturnMode::Unchanged)
        .build();

    let select = op.select.unwrap();
    assert_eq!(
        select.return_mode,
        crate::query::write::UpdateReturnMode::Unchanged
    );
}

#[test]
fn test_update_select_with_fields() {
    let op = write::update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("name", "Updated").set("status", "active"))
        .returning_fields(UpdateReturnMode::Changed, ["id", "name", "status"])
        .build();

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
    // Roundtrip tests validate serde — keep json! for the expected wire form.
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
    let op = write::update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("name", "Updated"))
        .build();

    assert!(op.select.is_none());
}

#[test]
fn test_update_select_serializes_without_optional_fields() {
    let op = write::update("users")
        .set(doc().set("status", "active"))
        .returning(UpdateReturnMode::Changed)
        .build();
    let serialized = serde_json::to_string(&op).unwrap();

    assert!(serialized.contains("select"));
    assert!(serialized.contains("changed"));
    assert!(!serialized.contains("fields"));
}

#[test]
fn test_update_select_default_mode() {
    // The builder always produces an explicit mode, so we test that
    // the wire DTO defaults to Changed when no mode is specified.
    // This must remain json!-based: it validates serde default behavior.
    let json = json!({
        "update": "users",
        "set": {
            "status": "active"
        },
        "select": {}
    });

    let op: UpdateOp = serde_json::from_value(json).unwrap();

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
    let op = write::upsert("users")
        .key(doc().set("id", 1_i64))
        .value(doc().set("name", "Alice").set("email", "alice@example.com"))
        .build();

    assert_eq!(op.set, TableRef::new("users"));
    assert_eq!(op.key["id"], 1);
    assert_eq!(op.value["name"], "Alice");
    assert_eq!(op.value["email"], "alice@example.com");
}

#[test]
fn test_set_by_unique_field() {
    let op = write::upsert("users")
        .key(doc().set("email", "alice@example.com"))
        .value(doc().set("name", "Alice Updated"))
        .build();

    assert_eq!(op.set, TableRef::new("users"));
    assert_eq!(op.key["email"], "alice@example.com");
    assert_eq!(op.value["name"], "Alice Updated");
}

#[test]
fn test_set_composite_key() {
    let op = write::upsert("order_items")
        .key(doc().set("order_id", 1_i64).set("product_id", 5_i64))
        .value(doc().set("qty", 3_i64).set("price", 19.99))
        .build();

    assert_eq!(op.set, TableRef::new("order_items"));
    assert_eq!(op.key["order_id"], 1);
    assert_eq!(op.key["product_id"], 5);
    assert_eq!(op.value["qty"], 3);
}

#[test]
fn test_set_roundtrip() {
    // Roundtrip tests validate serde — keep json! for the expected wire form.
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
    let op = write::delete("users")
        .where_(filter::eq("status", "inactive"))
        .build();

    assert_eq!(op.delete_from, TableRef::new("users"));
}

#[test]
fn test_delete_with_complex_filter() {
    let op = write::delete("logs")
        .where_(filter::lt("created_at", "2023-01-01").and(filter::eq("archived", true)))
        .build();

    assert_eq!(op.delete_from, TableRef::new("logs"));
}

#[test]
fn test_delete_by_id() {
    let op = write::delete("users")
        .where_(filter::eq("id", 42_i64))
        .build();

    assert_eq!(op.delete_from, TableRef::new("users"));
}

#[test]
fn test_delete_roundtrip() {
    // Roundtrip tests validate serde — keep json! for the expected wire form.
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
// These tests validate serde error handling — json! is the correct tool.

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
    let op = write::insert("users")
        .row(
            doc()
                .set("name", "Alice")
                .set_value("email", mpack!(null)),
        )
        .build();

    assert!(op.values[0]["email"].is_null());
}

#[test]
fn test_insert_with_special_characters() {
    let op = write::insert("users")
        .row(
            doc()
                .set("name", "O'Brien")
                .set("bio", "Line1\nLine2\tTabbed")
                .set("emoji", "\u{1f600}\u{1f389}"),
        )
        .build();

    assert_eq!(op.values[0]["name"], "O'Brien");
    assert_eq!(op.values[0]["emoji"], "\u{1f600}\u{1f389}");
}

#[test]
fn test_insert_with_numbers() {
    let op = write::insert("products")
        .row(
            doc()
                .set("id", 1_i64)
                .set("price", 99.99)
                .set("stock", 100_i64)
                .set("weight", 1.5e-3),
        )
        .build();

    assert_eq!(op.values[0]["id"], 1);
    assert_eq!(op.values[0]["price"], 99.99);
    assert_eq!(op.values[0]["stock"], 100);
}

#[test]
fn test_insert_with_boolean() {
    let op = write::insert("flags")
        .row(doc().set("active", true).set("deleted", false))
        .build();

    assert_eq!(op.values[0]["active"], true);
    assert_eq!(op.values[0]["deleted"], false);
}
