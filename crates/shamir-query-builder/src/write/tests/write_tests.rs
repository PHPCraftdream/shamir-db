//! Tests for the `write` module — Doc, Insert, Update, Upsert, Delete.
//!
//! Each builder is verified against the expected `QueryValue` structure and
//! round-tripped through msgpack (`rmp_serde::to_vec_named` /
//! `rmp_serde::from_slice`).
//!
//! Multi-field `Doc` assertions check individual keys rather than full
//! structural equality because insertion order is preserved but we do not
//! rely on a specific key ordering for the assertions.

use shamir_query_types::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::filter;
use crate::val::*;
use crate::write::*;

// ── helpers ─────────────────────────────────────────────────────────

/// Serialize a DTO to msgpack, assert structural equality via `QueryValue`,
/// then round-trip back and compare.
fn assert_dto_wire<T>(dto: &T, expected: &QueryValue)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let bytes = rmp_serde::to_vec_named(dto).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got, *expected, "wire msgpack mismatch");
    let back: T = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, *dto, "round-trip mismatch");
}

// ============================================================================
// Doc
// ============================================================================

#[test]
fn doc_empty() {
    let d = doc().build();
    assert_eq!(d, mpack!({}));
}

#[test]
fn doc_literal_fields() {
    let d = doc().set("name", "Alice").set("age", 30).build();
    assert_eq!(d["name"], QueryValue::Str("Alice".to_string()));
    assert_eq!(d["age"], QueryValue::Int(30));
}

#[test]
fn doc_nested_literal() {
    let d = doc()
        .set_value("address", mpack!({"city": "NY", "zip": "10001"}))
        .build();
    assert_eq!(d["address"]["city"], QueryValue::Str("NY".to_string()));
    assert_eq!(d["address"]["zip"], QueryValue::Str("10001".to_string()));
}

#[test]
fn doc_set_expr_fn_call() {
    let d = doc()
        .set("email_norm", func("strings/lower", [col("email")]))
        .build();
    assert_eq!(
        d["email_norm"],
        mpack!({
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
    assert_eq!(d["copy"], mpack!({"$ref": ["source"]}));
}

#[test]
fn doc_set_expr_query_ref() {
    let d = doc().set("user_id", qref("users", "[0].id")).build();
    assert_eq!(d["user_id"], mpack!({"$query": "@users", "path": "[0].id"}));
}

#[test]
fn doc_into_value() {
    let v: QueryValue = doc().set("x", 1).into();
    assert_eq!(v["x"], QueryValue::Int(1));
}

// ============================================================================
// Insert
// ============================================================================

#[test]
fn insert_single_row_literal() {
    let op = insert("users").row(mpack!({"name": "Alice"})).build();
    let expected = mpack!({
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
    // Check structure per-field.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["insert_into"], QueryValue::Str("users".to_string()));
    let row = &got["values"][0];
    assert_eq!(row["name"], QueryValue::Str("Alice".to_string()));
    assert_eq!(row["age"], QueryValue::Int(25));
    // Round-trip the DTO.
    let back: InsertOp = rmp_serde::from_slice(&bytes).unwrap();
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
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["insert_into"], QueryValue::Str("users".to_string()));
    let row = &got["values"][0];
    assert_eq!(row["email"], QueryValue::Str("A@X.COM".to_string()));
    assert_eq!(
        row["email_norm"],
        mpack!({
            "$fn": {
                "name": "strings/lower",
                "args": [{"$ref": ["email"]}]
            }
        })
    );
    // Round-trip.
    let back: InsertOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn insert_multiple_rows() {
    let op = insert("items")
        .rows(vec![mpack!({"a": 1}), mpack!({"a": 2})])
        .build();
    assert_eq!(op.values.len(), 2);
    let expected = mpack!({
        "insert_into": "items",
        "values": [{"a": 1}, {"a": 2}]
    });
    assert_dto_wire(&op, &expected);
}

#[test]
fn insert_with_repo() {
    let op = Insert::with_repo("hot", "sessions")
        .row(mpack!({"token": "abc"}))
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["insert_into"], mpack!(["hot", "sessions"]));
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
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["update"], QueryValue::Str("users".to_string()));
    assert!(matches!(got["where"], QueryValue::Map(_)));
    assert_eq!(got["set"]["name"], QueryValue::Str("Bob".to_string()));
    // No select key when not set.
    if let QueryValue::Map(ref m) = got {
        assert!(!m.contains_key("select"));
    }
    // Round-trip.
    let back: UpdateOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn update_with_returning_all() {
    let op = update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("active", false))
        .returning(UpdateReturnMode::All)
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(
        got["select"]["return_mode"],
        QueryValue::Str("all".to_string())
    );
    if let QueryValue::Map(ref m) = got["select"] {
        assert!(!m.contains_key("fields"));
    }
    let back: UpdateOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn update_with_returning_fields() {
    let op = update("users")
        .where_(filter::eq("id", 1_i64))
        .set(doc().set("name", "X"))
        .returning_fields(UpdateReturnMode::Changed, ["id", "name"])
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(
        got["select"]["return_mode"],
        QueryValue::Str("changed".to_string())
    );
    assert_eq!(got["select"]["fields"], mpack!(["id", "name"]));
    let back: UpdateOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn update_with_repo() {
    let op = Update::with_repo("hot", "sessions")
        .set(mpack!({"renewed": true}))
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["update"], mpack!(["hot", "sessions"]));
}

#[test]
fn update_no_where() {
    // An update without where is valid (updates all records).
    let op = update("users").set(doc().set("active", false)).build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    if let QueryValue::Map(ref m) = got {
        assert!(!m.contains_key("where"));
    }
}

// ============================================================================
// Upsert (SetOp)
// ============================================================================

#[test]
fn upsert_basic() {
    let op = upsert("cache")
        .key(mpack!("session:abc"))
        .value(doc().set("data", "payload"))
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["set"], QueryValue::Str("cache".to_string()));
    assert_eq!(got["key"], QueryValue::Str("session:abc".to_string()));
    assert_eq!(got["value"]["data"], QueryValue::Str("payload".to_string()));
    let back: SetOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn upsert_with_repo() {
    let op = Upsert::with_repo("hot", "kv")
        .key(mpack!("k1"))
        .value(mpack!(42))
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["set"], mpack!(["hot", "kv"]));
    assert_eq!(got["key"], QueryValue::Str("k1".to_string()));
    assert_eq!(got["value"], QueryValue::Int(42));
    let back: SetOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn upsert_with_doc_value() {
    let op = upsert("users")
        .key(mpack!({"email": "a@x.com"}))
        .value(doc().set("name", "Alice").set("age", 30))
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["value"]["name"], QueryValue::Str("Alice".to_string()));
    assert_eq!(got["value"]["age"], QueryValue::Int(30));
}

// ============================================================================
// Delete
// ============================================================================

#[test]
fn delete_basic() {
    let op = delete("sessions")
        .where_(filter::eq("expired", true))
        .build();
    let expected = mpack!({
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
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["delete_from"], mpack!(["hot", "sessions"]));
}

#[test]
fn delete_complex_where() {
    use crate::filter::FilterExt;

    let f = filter::eq("status", "inactive").and(filter::lt("last_seen", 1000_i64));
    let op = delete("users").where_(f).build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(got["delete_from"], QueryValue::Str("users".to_string()));
    // The where clause is an And with two children.
    assert_eq!(got["where"]["op"], QueryValue::Str("and".to_string()));
    let back: DeleteOp = rmp_serde::from_slice(&bytes).unwrap();
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
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    let row = &got["values"][0];
    assert_eq!(row["total"], QueryValue::Int(100));
    assert_eq!(
        row["user_id"],
        mpack!({"$query": "@users", "path": "[0].id"})
    );
    let back: InsertOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn upsert_with_computed_value() {
    let op = upsert("profiles")
        .key(mpack!("user:1"))
        .value(
            doc()
                .set("raw_name", "ALICE")
                .set("name", func("strings/lower", [col("raw_name")])),
        )
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(
        got["value"]["raw_name"],
        QueryValue::Str("ALICE".to_string())
    );
    assert_eq!(
        got["value"]["name"],
        mpack!({
            "$fn": {
                "name": "strings/lower",
                "args": [{"$ref": ["raw_name"]}]
            }
        })
    );
}
