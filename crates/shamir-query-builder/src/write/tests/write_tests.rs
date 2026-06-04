//! Tests for the `write` module — Doc, Insert, Update, Upsert, Delete.
//!
//! Each builder is verified against exact wire JSON and round-tripped
//! through serde where the DTO supports it.
//!
//! **Note:** `serde_json` `preserve_order` is NOT enabled in this
//! workspace, so multi-field `Doc` assertions check individual keys
//! rather than relying on object-level equality with a `json!()` literal
//! whose key order may differ.

use serde_json::{json, Value};
use shamir_query_types::write::{DeleteOp, InsertOp, SetOp, UpdateOp};

use crate::filter;
use crate::val::*;
use crate::write::*;

// ── helpers ─────────────────────────────────────────────────────────

/// Serialize a DTO, assert equality, then round-trip back.
fn assert_dto_wire<T>(dto: &T, expected: &Value)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let got = serde_json::to_value(dto).unwrap();
    assert_eq!(got, *expected, "wire JSON mismatch");
    let back: T = serde_json::from_value(got).unwrap();
    assert_eq!(back, *dto, "round-trip mismatch");
}

// ============================================================================
// Doc
// ============================================================================

#[test]
fn doc_empty() {
    let d = doc().build();
    assert_eq!(d, json!({}));
}

#[test]
fn doc_literal_fields() {
    let d = doc().set("name", "Alice").set("age", 30).build();
    assert_eq!(d["name"], json!("Alice"));
    assert_eq!(d["age"], json!(30));
}

#[test]
fn doc_nested_literal() {
    let d = doc()
        .set_json("address", json!({"city": "NY", "zip": "10001"}))
        .build();
    assert_eq!(d["address"]["city"], json!("NY"));
    assert_eq!(d["address"]["zip"], json!("10001"));
}

#[test]
fn doc_set_expr_fn_call() {
    let d = doc()
        .set("email_norm", func("strings/lower", [col("email")]))
        .build();
    assert_eq!(
        d["email_norm"],
        json!({
            "$fn": {
                "name": "strings/lower",
                "args": [{"$ref": ["email"]}]
            }
        })
    );
}

#[test]
fn doc_set_expr_field_ref() {
    let d = doc().set("copy", col("source")).build();
    assert_eq!(d["copy"], json!({"$ref": ["source"]}));
}

#[test]
fn doc_set_expr_query_ref() {
    let d = doc().set("user_id", qref("users", "[0].id")).build();
    assert_eq!(d["user_id"], json!({"$query": "@users", "path": "[0].id"}));
}

#[test]
fn doc_into_value() {
    let v: Value = doc().set("x", 1).into();
    assert_eq!(v["x"], json!(1));
}

// ============================================================================
// Insert
// ============================================================================

#[test]
fn insert_single_row_literal() {
    let op = insert("users").row(json!({"name": "Alice"})).build();
    let expected = json!({
        "insert_into": "users",
        "values": [{"name": "Alice"}]
    });
    assert_dto_wire(&op, &expected);
}

#[test]
fn insert_with_doc() {
    let op = insert("users")
        .row(doc().set("name", "Alice").set("age", 25))
        .build();
    // Check structure per-field (no preserve_order).
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["insert_into"], json!("users"));
    let row = &got["values"][0];
    assert_eq!(row["name"], json!("Alice"));
    assert_eq!(row["age"], json!(25));
    // Round-trip the DTO.
    let back: InsertOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

/// The key test from the task spec: computed write value with `$fn`.
#[test]
fn insert_computed_fn_value() {
    let op = insert("users")
        .row(
            doc()
                .set("email", "A@X.COM")
                .set("email_norm", func("strings/lower", [col("email")])),
        )
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["insert_into"], json!("users"));
    let row = &got["values"][0];
    assert_eq!(row["email"], json!("A@X.COM"));
    assert_eq!(
        row["email_norm"],
        json!({
            "$fn": {
                "name": "strings/lower",
                "args": [{"$ref": ["email"]}]
            }
        })
    );
    // Round-trip.
    let back: InsertOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
fn insert_multiple_rows() {
    let op = insert("items")
        .rows(vec![json!({"a": 1}), json!({"a": 2})])
        .build();
    assert_eq!(op.values.len(), 2);
    let expected = json!({
        "insert_into": "items",
        "values": [{"a": 1}, {"a": 2}]
    });
    assert_dto_wire(&op, &expected);
}

#[test]
fn insert_with_repo() {
    let op = Insert::with_repo("hot", "sessions")
        .row(json!({"token": "abc"}))
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["insert_into"], json!(["hot", "sessions"]));
}

// ============================================================================
// Update
// ============================================================================

#[test]
fn update_basic() {
    let op = update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("name", "Bob"))
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["update"], json!("users"));
    assert!(got["where"].is_object());
    assert_eq!(got["set"]["name"], json!("Bob"));
    // No select key when not set.
    assert!(got.get("select").is_none());
    // Round-trip.
    let back: UpdateOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
fn update_with_returning_all() {
    let op = update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("active", false))
        .returning(UpdateReturnMode::All)
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["select"]["return_mode"], json!("all"));
    assert!(got["select"].get("fields").is_none());
    let back: UpdateOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
fn update_with_returning_fields() {
    let op = update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("name", "X"))
        .returning_fields(UpdateReturnMode::Changed, ["id", "name"])
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["select"]["return_mode"], json!("changed"));
    assert_eq!(got["select"]["fields"], json!(["id", "name"]));
    let back: UpdateOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
fn update_with_repo() {
    let op = Update::with_repo("hot", "sessions")
        .set(json!({"renewed": true}))
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["update"], json!(["hot", "sessions"]));
}

#[test]
fn update_no_where() {
    // An update without where is valid (updates all records).
    let op = update("users").set(doc().set("active", false)).build();
    let got = serde_json::to_value(&op).unwrap();
    assert!(got.get("where").is_none());
}

// ============================================================================
// Upsert (SetOp)
// ============================================================================

#[test]
fn upsert_basic() {
    let op = upsert("cache")
        .key(json!("session:abc"))
        .value(doc().set("data", "payload"))
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["set"], json!("cache"));
    assert_eq!(got["key"], json!("session:abc"));
    assert_eq!(got["value"]["data"], json!("payload"));
    let back: SetOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
fn upsert_with_repo() {
    let op = Upsert::with_repo("hot", "kv")
        .key(json!("k1"))
        .value(json!(42))
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["set"], json!(["hot", "kv"]));
    assert_eq!(got["key"], json!("k1"));
    assert_eq!(got["value"], json!(42));
    let back: SetOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
fn upsert_with_doc_value() {
    let op = upsert("users")
        .key(json!({"email": "a@x.com"}))
        .value(doc().set("name", "Alice").set("age", 30))
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["value"]["name"], json!("Alice"));
    assert_eq!(got["value"]["age"], json!(30));
}

// ============================================================================
// Delete
// ============================================================================

#[test]
fn delete_basic() {
    let op = delete("sessions")
        .where_(filter::eq("expired", true))
        .build();
    let expected = json!({
        "delete_from": "sessions",
        "where": {
            "op": "eq",
            "field": ["expired"],
            "value": true
        }
    });
    assert_dto_wire(&op, &expected);
}

#[test]
fn delete_with_repo() {
    let op = Delete::with_repo("hot", "sessions")
        .where_(filter::eq("expired", true))
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["delete_from"], json!(["hot", "sessions"]));
}

#[test]
fn delete_complex_where() {
    use crate::filter::FilterExt;

    let f = filter::eq("status", "inactive").and(filter::lt("last_seen", 1000_i64));
    let op = delete("users").where_(f).build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["delete_from"], json!("users"));
    // The where clause is an And with two children.
    assert_eq!(got["where"]["op"], json!("and"));
    let back: DeleteOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
#[should_panic(expected = "Delete::build() requires a where clause")]
fn delete_without_where_panics() {
    let _ = delete("users").build();
}

// ============================================================================
// Cross-query ref in a write value ($query)
// ============================================================================

#[test]
fn insert_with_query_ref_value() {
    let op = insert("orders")
        .row(
            doc()
                .set("total", 100)
                .set("user_id", qref("users", "[0].id")),
        )
        .build();
    let got = serde_json::to_value(&op).unwrap();
    let row = &got["values"][0];
    assert_eq!(row["total"], json!(100));
    assert_eq!(
        row["user_id"],
        json!({"$query": "@users", "path": "[0].id"})
    );
    let back: InsertOp = serde_json::from_value(got).unwrap();
    assert_eq!(back, op);
}

#[test]
fn upsert_with_computed_value() {
    let op = upsert("profiles")
        .key(json!("user:1"))
        .value(
            doc()
                .set("raw_name", "ALICE")
                .set("name", func("strings/lower", [col("raw_name")])),
        )
        .build();
    let got = serde_json::to_value(&op).unwrap();
    assert_eq!(got["value"]["raw_name"], json!("ALICE"));
    assert_eq!(
        got["value"]["name"],
        json!({
            "$fn": {
                "name": "strings/lower",
                "args": [{"$ref": ["raw_name"]}]
            }
        })
    );
}
