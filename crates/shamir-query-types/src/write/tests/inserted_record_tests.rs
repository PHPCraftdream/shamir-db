//! Serialization golden tests for `InsertedRecord`.
//!
//! These cover the UPDATE-RETURNING and SET-upsert result shapes.
//! Every test asserts that the msgpack (named-map) wire output is deterministic
//! and matches the expected sorted-key output built via `mpack!`.

use shamir_types::mpack;
use shamir_types::types::common::TMap;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{QueryValue, Value};

use crate::write::InsertedRecord;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_base_map() -> TMap<String, Value<String>> {
    let mut m: TMap<String, Value<String>> = TMap::default();
    m.insert("name".to_string(), Value::Str("widget".to_string()));
    m.insert("qty".to_string(), Value::Int(42));
    m
}

// ---------------------------------------------------------------------------
// 1. UPDATE-RETURNING — no _id, just data fields + overlay
// ---------------------------------------------------------------------------

/// Base fields only, no overlay, no _id.
/// Expected sorted order: name, qty.
#[test]
fn update_returning_base_only() {
    let m = make_base_map();
    let rec = InsertedRecord {
        id: None,
        fields: QueryValue::Map(m),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    // Expected: map sorted by key — name < qty
    let expected = mpack!({ "name": "widget", "qty": 42_i64 });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(bytes, expected_bytes, "UPDATE-RETURNING base only");
}

/// Base fields + overlay with a NEW field not in the base record.
/// Expected sorted order: color, name, qty.
#[test]
fn update_returning_with_new_overlay_field() {
    let mut m = make_base_map();
    // Overlay: change qty, add brand-new "color" field.
    m.insert("qty".to_string(), Value::Int(99));
    m.insert("color".to_string(), Value::Str("red".to_string()));

    let rec = InsertedRecord {
        id: None,
        fields: QueryValue::Map(m),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    // Expected sorted: color < name < qty
    let expected = mpack!({ "color": "red", "name": "widget", "qty": 99_i64 });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(
        bytes, expected_bytes,
        "UPDATE-RETURNING with new overlay field"
    );
}

// ---------------------------------------------------------------------------
// 2. SET-UPDATE — no _id, data fields + _created: false
// ---------------------------------------------------------------------------

/// SET update path: fields + _created=false, no _id.
/// Expected sorted order: _created, name, qty.
#[test]
fn set_update_with_created_false() {
    let mut m = make_base_map();
    m.insert("_created".to_string(), Value::Bool(false));

    let rec = InsertedRecord {
        id: None,
        fields: QueryValue::Map(m),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    // Expected sorted: _created < name < qty
    let expected = mpack!({ "_created": false, "name": "widget", "qty": 42_i64 });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(bytes, expected_bytes, "SET-UPDATE _created=false");
}

// ---------------------------------------------------------------------------
// 3. SET-INSERT (map) — _id injected by Direct, data fields + _created: true
// ---------------------------------------------------------------------------

/// SET insert path (map value): _id injected, _created=true.
/// Expected sorted order: _created, _id, name, qty.
#[test]
fn set_insert_map_with_id_and_created() {
    let id = RecordId::system("set-ins-01");
    let id_str = id.to_string();
    let mut m = make_base_map();
    m.insert("_created".to_string(), Value::Bool(true));

    let rec = InsertedRecord {
        id: Some(id),
        fields: QueryValue::Map(m),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    // Expected sorted: _created < _id < name < qty
    let expected = mpack!({
        "_created": true,
        "_id": @ QueryValue::Str(id_str),
        "name": "widget",
        "qty": 42_i64
    });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(bytes, expected_bytes, "SET-INSERT map with id");
}

// ---------------------------------------------------------------------------
// 4. SET-INSERT (non-map _value) — _id injected, _value + _created
// ---------------------------------------------------------------------------

/// SET insert path (non-map value): wraps as {_value: 42, _created: true},
/// Direct injects _id. Expected sorted: _created, _id, _value.
#[test]
fn set_insert_non_map_value_with_id_and_created() {
    let id = RecordId::system("set-ins-02");
    let id_str = id.to_string();
    let mut m: TMap<String, Value<String>> = TMap::default();
    m.insert("_value".to_string(), Value::Int(42));
    m.insert("_created".to_string(), Value::Bool(true));

    let rec = InsertedRecord {
        id: Some(id),
        fields: QueryValue::Map(m),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    // Expected sorted: _created < _id < _value
    let expected = mpack!({
        "_created": true,
        "_id": @ QueryValue::Str(id_str),
        "_value": 42_i64
    });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(bytes, expected_bytes, "SET-INSERT non-map value with id");
}

// ---------------------------------------------------------------------------
// 5. Synthetic-key interleave stress: _created sits between user keys
// ---------------------------------------------------------------------------

/// Keys that interleave with underscore-prefixed synthetic keys.
/// Sorted order: _created, a_field, name — verifies the sort
/// matches alphabetical order exactly.
#[test]
fn synthetic_key_interleave_order() {
    let mut m: TMap<String, Value<String>> = TMap::default();
    m.insert("name".to_string(), Value::Str("x".to_string()));
    m.insert("a_field".to_string(), Value::Int(1));
    m.insert("_created".to_string(), Value::Bool(false));

    let rec = InsertedRecord {
        id: None,
        fields: QueryValue::Map(m),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    // Expected sorted: _created < a_field < name
    let expected = mpack!({ "_created": false, "a_field": 1_i64, "name": "x" });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(bytes, expected_bytes, "synthetic key interleave");
}

/// Same interleave but with _id injected (SET-INSERT scenario).
/// Sorted order: _created, _id, a_field, name.
#[test]
fn synthetic_key_interleave_with_id() {
    let id = RecordId::system("interleave-01");
    let id_str = id.to_string();
    let mut m: TMap<String, Value<String>> = TMap::default();
    m.insert("name".to_string(), Value::Str("x".to_string()));
    m.insert("a_field".to_string(), Value::Int(1));
    m.insert("_created".to_string(), Value::Bool(true));

    let rec = InsertedRecord {
        id: Some(id),
        fields: QueryValue::Map(m),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    // Expected sorted: _created, _id, a_field, name
    let expected = mpack!({
        "_created": true,
        "_id": @ QueryValue::Str(id_str),
        "a_field": 1_i64,
        "name": "x"
    });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(bytes, expected_bytes, "synthetic key interleave with id");
}

// ---------------------------------------------------------------------------
// 6. id: None, non-map fields — edge case (serializes the value directly)
// ---------------------------------------------------------------------------

/// No id, non-map value — serializes the value directly (not wrapped in a map).
#[test]
fn no_id_non_map_value_direct_serialization() {
    let rec = InsertedRecord {
        id: None,
        fields: QueryValue::Int(42),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    let expected = mpack!(42_i64);
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(
        bytes, expected_bytes,
        "no-id non-map value serializes directly"
    );
}
