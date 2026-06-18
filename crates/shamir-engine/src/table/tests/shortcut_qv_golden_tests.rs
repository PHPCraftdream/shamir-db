//! Golden tests: shortcut read paths (MIN/MAX/COUNT) must produce
//! correct wire output after the QueryValue migration.
//!
//! Each test constructs a `Direct(QueryValue::Map)` row (the new canonical
//! shape) and verifies that the JSON and msgpack serialization matches the
//! expected logical value.  The former `Json`-vs-`Direct` byte-identity
//! assertions are superseded — `Json` was removed in Stage C.

use shamir_query_types::read::QueryRecord;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

// ── helpers ─────────────────────────────────────────────────────────────

/// Assert that a `Direct` row serialises to the given expected JSON value.
fn assert_json_value(row: &QueryRecord, expected: serde_json::Value) {
    let bytes = serde_json::to_vec(row).expect("json ser");
    let got: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        got, expected,
        "JSON value mismatch:\n  got: {got}\n  expected: {expected}"
    );
}

// ── MIN shortcut ────────────────────────────────────────────────────────

#[test]
fn min_shortcut_int_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min_score".to_string(), QueryValue::Int(42));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "min_score": 42 }));
}

#[test]
fn min_shortcut_float_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min_temp".to_string(), QueryValue::F64(3.5));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "min_temp": 3.5 }));
}

#[test]
fn min_shortcut_string_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min_name".to_string(), QueryValue::Str("alice".to_string()));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "min_name": "alice" }));
}

#[test]
fn min_shortcut_null_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min".to_string(), QueryValue::Null);
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "min": null }));
}

// ── MAX shortcut ────────────────────────────────────────────────────────

#[test]
fn max_shortcut_int_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("max_score".to_string(), QueryValue::Int(999));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "max_score": 999 }));
}

#[test]
fn max_shortcut_null_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("max".to_string(), QueryValue::Null);
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "max": null }));
}

// ── COUNT shortcut ──────────────────────────────────────────────────────

#[test]
fn count_shortcut_zero_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("count".to_string(), QueryValue::Int(0));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "count": 0 }));
}

#[test]
fn count_shortcut_typical_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("count".to_string(), QueryValue::Int(42));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "count": 42 }));
}

#[test]
fn count_shortcut_large_wire_identical() {
    // A count near i64::MAX but still representable.
    let count: i64 = i64::MAX;
    let mut obj = new_map_wc(1);
    obj.insert("total".to_string(), QueryValue::Int(count));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "total": count }));
}

#[test]
fn count_shortcut_with_alias_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("num_users".to_string(), QueryValue::Int(100));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_json_value(&row, serde_json::json!({ "num_users": 100 }));
}

// ── Temporal metadata (_version / _ts) ──────────────────────────────────

#[test]
fn temporal_version_metadata_wire_identical() {
    let version: i64 = 7;
    let ts: i64 = 1_718_400_000_000; // ~2024

    let mut obj = new_map_wc(3);
    obj.insert("name".to_string(), QueryValue::Str("alice".to_string()));
    obj.insert("_version".to_string(), QueryValue::Int(version));
    obj.insert("_ts".to_string(), QueryValue::Int(ts));
    let row = QueryRecord::Direct(QueryValue::Map(obj));

    let bytes = serde_json::to_vec(&row).expect("json ser");
    let got: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(got["name"], serde_json::json!("alice"));
    assert_eq!(got["_version"], serde_json::json!(version));
    assert_eq!(got["_ts"], serde_json::json!(ts));
}

#[test]
fn temporal_version_null_ts_wire_identical() {
    let version: i64 = 3;

    let mut obj = new_map_wc(2);
    obj.insert("_version".to_string(), QueryValue::Int(version));
    obj.insert("_ts".to_string(), QueryValue::Null);
    let row = QueryRecord::Direct(QueryValue::Map(obj));

    let bytes = serde_json::to_vec(&row).expect("json ser");
    let got: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(got["_version"], serde_json::json!(version));
    assert_eq!(got["_ts"], serde_json::Value::Null);
}
