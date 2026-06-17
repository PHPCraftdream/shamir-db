//! Byte-identity golden tests for `InsertedRecord::Direct` vs `InsertedRecord::Json`.
//!
//! These cover the UPDATE-RETURNING and SET-upsert result shapes that
//! were migrated from `InsertedRecord::Json` to `InsertedRecord::Direct`.
//! Every test asserts that the msgpack (named-map) wire output is
//! byte-identical between the two variants.

use serde_json::json;
use shamir_types::types::common::TMap;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{QueryValue, Value};

use crate::write::InsertedRecord;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `Direct` and its `Json` twin, assert their `rmp_serde::to_vec_named`
/// outputs are byte-identical.
fn assert_byte_identical(direct: &InsertedRecord, json_rec: &InsertedRecord) {
    let direct_bytes = rmp_serde::to_vec_named(direct).expect("direct serialize");
    let json_bytes = rmp_serde::to_vec_named(json_rec).expect("json serialize");
    assert_eq!(
        direct_bytes, json_bytes,
        "msgpack bytes must be identical for Direct and Json variants"
    );
}

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
#[test]
fn update_returning_base_only() {
    let m = make_base_map();
    let direct = InsertedRecord::Direct {
        id: None,
        fields: QueryValue::Map(m),
    };
    // Json twin: serde_json::Map sorts keys alphabetically (BTreeMap).
    let mut jm = serde_json::Map::new();
    jm.insert("name".to_string(), json!("widget"));
    jm.insert("qty".to_string(), json!(42));
    let json_rec = InsertedRecord::Json(serde_json::Value::Object(jm));
    assert_byte_identical(&direct, &json_rec);
}

/// Base fields + overlay with a NEW field not in the base record.
#[test]
fn update_returning_with_new_overlay_field() {
    let mut m = make_base_map();
    // Overlay: change qty, add brand-new "color" field.
    m.insert("qty".to_string(), Value::Int(99));
    m.insert("color".to_string(), Value::Str("red".to_string()));

    let direct = InsertedRecord::Direct {
        id: None,
        fields: QueryValue::Map(m),
    };
    // Json twin.
    let mut jm = serde_json::Map::new();
    jm.insert("color".to_string(), json!("red"));
    jm.insert("name".to_string(), json!("widget"));
    jm.insert("qty".to_string(), json!(99));
    let json_rec = InsertedRecord::Json(serde_json::Value::Object(jm));
    assert_byte_identical(&direct, &json_rec);
}

// ---------------------------------------------------------------------------
// 2. SET-UPDATE — no _id, data fields + _created: false
// ---------------------------------------------------------------------------

/// SET update path: fields + _created=false, no _id.
#[test]
fn set_update_with_created_false() {
    let mut m = make_base_map();
    m.insert("_created".to_string(), Value::Bool(false));

    let direct = InsertedRecord::Direct {
        id: None,
        fields: QueryValue::Map(m),
    };
    // Json twin: keys sorted => _created, name, qty
    let mut jm = serde_json::Map::new();
    jm.insert("_created".to_string(), json!(false));
    jm.insert("name".to_string(), json!("widget"));
    jm.insert("qty".to_string(), json!(42));
    let json_rec = InsertedRecord::Json(serde_json::Value::Object(jm));
    assert_byte_identical(&direct, &json_rec);
}

// ---------------------------------------------------------------------------
// 3. SET-INSERT (map) — _id injected by Direct, data fields + _created: true
// ---------------------------------------------------------------------------

/// SET insert path (map value): _id injected, _created=true.
#[test]
fn set_insert_map_with_id_and_created() {
    let id = RecordId::system("set-ins-01");
    let id_str = id.to_string();
    let mut m = make_base_map();
    m.insert("_created".to_string(), Value::Bool(true));

    let direct = InsertedRecord::Direct {
        id: Some(id),
        fields: QueryValue::Map(m),
    };
    // Json twin: keys sorted => _created, _id, name, qty
    let mut jm = serde_json::Map::new();
    jm.insert("_created".to_string(), json!(true));
    jm.insert("_id".to_string(), json!(id_str));
    jm.insert("name".to_string(), json!("widget"));
    jm.insert("qty".to_string(), json!(42));
    let json_rec = InsertedRecord::Json(serde_json::Value::Object(jm));
    assert_byte_identical(&direct, &json_rec);
}

// ---------------------------------------------------------------------------
// 4. SET-INSERT (non-map _value) — _id injected by Direct, _value + _created
// ---------------------------------------------------------------------------

/// SET insert path (non-map value): wraps as {_value: 42}, Direct injects _id.
#[test]
fn set_insert_non_map_value_with_id_and_created() {
    let id = RecordId::system("set-ins-02");
    let id_str = id.to_string();
    let mut m: TMap<String, Value<String>> = TMap::default();
    m.insert("_value".to_string(), Value::Int(42));
    m.insert("_created".to_string(), Value::Bool(true));

    let direct = InsertedRecord::Direct {
        id: Some(id),
        fields: QueryValue::Map(m),
    };
    // Json twin: keys sorted => _created, _id, _value
    let mut jm = serde_json::Map::new();
    jm.insert("_created".to_string(), json!(true));
    jm.insert("_id".to_string(), json!(id_str));
    jm.insert("_value".to_string(), json!(42));
    let json_rec = InsertedRecord::Json(serde_json::Value::Object(jm));
    assert_byte_identical(&direct, &json_rec);
}

// ---------------------------------------------------------------------------
// 5. Synthetic-key interleave stress: _created sits between user keys
// ---------------------------------------------------------------------------

/// Keys that interleave with underscore-prefixed synthetic keys.
/// Sorted order: _created, a_field, name — verifies Direct's sort
/// matches serde_json::Map's BTree order exactly.
#[test]
fn synthetic_key_interleave_order() {
    let mut m: TMap<String, Value<String>> = TMap::default();
    m.insert("name".to_string(), Value::Str("x".to_string()));
    m.insert("a_field".to_string(), Value::Int(1));
    m.insert("_created".to_string(), Value::Bool(false));

    let direct = InsertedRecord::Direct {
        id: None,
        fields: QueryValue::Map(m),
    };
    // Json twin: _created < a_field < name
    let mut jm = serde_json::Map::new();
    jm.insert("_created".to_string(), json!(false));
    jm.insert("a_field".to_string(), json!(1));
    jm.insert("name".to_string(), json!("x"));
    let json_rec = InsertedRecord::Json(serde_json::Value::Object(jm));
    assert_byte_identical(&direct, &json_rec);
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

    let direct = InsertedRecord::Direct {
        id: Some(id),
        fields: QueryValue::Map(m),
    };
    // Json twin: _created, _id, a_field, name
    let mut jm = serde_json::Map::new();
    jm.insert("_created".to_string(), json!(true));
    jm.insert("_id".to_string(), json!(id_str));
    jm.insert("a_field".to_string(), json!(1));
    jm.insert("name".to_string(), json!("x"));
    let json_rec = InsertedRecord::Json(serde_json::Value::Object(jm));
    assert_byte_identical(&direct, &json_rec);
}

// ---------------------------------------------------------------------------
// 6. id: None, non-map fields — edge case (Direct emits value directly)
// ---------------------------------------------------------------------------

/// No id, non-map value — Direct serializes the value directly.
/// This would not normally occur in production, but covers the edge
/// case in the serializer for completeness.
#[test]
fn no_id_non_map_value_direct_serialization() {
    let direct = InsertedRecord::Direct {
        id: None,
        fields: QueryValue::Int(42),
    };
    // Json twin: just the number 42.
    let json_rec = InsertedRecord::Json(json!(42));
    assert_byte_identical(&direct, &json_rec);
}
