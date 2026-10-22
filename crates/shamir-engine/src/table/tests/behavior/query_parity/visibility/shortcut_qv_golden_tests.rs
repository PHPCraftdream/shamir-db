//! Golden tests: shortcut read paths (MIN/MAX/COUNT) must produce
//! correct wire output after the QueryValue migration.
//!
//! Each test constructs a `Direct(QueryValue::Map)` row (the new canonical
//! shape) and verifies that the msgpack serialization round-trips to the
//! expected logical values.

use shamir_query_types::read::QueryRecord;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

// ── helpers ─────────────────────────────────────────────────────────────

/// Assert that a `Direct` row round-trips through msgpack with the expected
/// field value. Deserialises using `rmp_serde::from_slice` to a `QueryValue`
/// map and checks the specific field.
fn assert_msgpack_field(row: &QueryRecord, field: &str, expected: QueryValue) {
    let bytes = rmp_serde::to_vec_named(row).expect("msgpack ser");
    let got: QueryValue = rmp_serde::from_slice(&bytes).expect("msgpack de");
    assert_eq!(
        got[field], expected,
        "msgpack field '{}' mismatch: got {:?}, expected {:?}",
        field, got[field], expected
    );
}

// ── MIN shortcut ────────────────────────────────────────────────────────

#[test]
fn min_shortcut_int_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min_score".to_string(), QueryValue::Int(42));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "min_score", QueryValue::Int(42));
}

#[test]
fn min_shortcut_float_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min_temp".to_string(), QueryValue::F64(3.5));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "min_temp", QueryValue::F64(3.5));
}

#[test]
fn min_shortcut_string_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min_name".to_string(), QueryValue::Str("alice".to_string()));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "min_name", QueryValue::Str("alice".to_string()));
}

#[test]
fn min_shortcut_null_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("min".to_string(), QueryValue::Null);
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "min", QueryValue::Null);
}

// ── MAX shortcut ────────────────────────────────────────────────────────

#[test]
fn max_shortcut_int_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("max_score".to_string(), QueryValue::Int(999));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "max_score", QueryValue::Int(999));
}

#[test]
fn max_shortcut_null_value_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("max".to_string(), QueryValue::Null);
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "max", QueryValue::Null);
}

// ── COUNT shortcut ──────────────────────────────────────────────────────

#[test]
fn count_shortcut_zero_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("count".to_string(), QueryValue::Int(0));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "count", QueryValue::Int(0));
}

#[test]
fn count_shortcut_typical_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("count".to_string(), QueryValue::Int(42));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "count", QueryValue::Int(42));
}

#[test]
fn count_shortcut_large_wire_identical() {
    // A count near i64::MAX but still representable.
    let count: i64 = i64::MAX;
    let mut obj = new_map_wc(1);
    obj.insert("total".to_string(), QueryValue::Int(count));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "total", QueryValue::Int(count));
}

#[test]
fn count_shortcut_with_alias_wire_identical() {
    let mut obj = new_map_wc(1);
    obj.insert("num_users".to_string(), QueryValue::Int(100));
    let row = QueryRecord::Direct(QueryValue::Map(obj));
    assert_msgpack_field(&row, "num_users", QueryValue::Int(100));
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

    let bytes = rmp_serde::to_vec_named(&row).expect("msgpack ser");
    let got: QueryValue = rmp_serde::from_slice(&bytes).expect("msgpack de");
    assert_eq!(got["name"], QueryValue::Str("alice".to_string()));
    assert_eq!(got["_version"], QueryValue::Int(version));
    assert_eq!(got["_ts"], QueryValue::Int(ts));
}

#[test]
fn temporal_version_null_ts_wire_identical() {
    let version: i64 = 3;

    let mut obj = new_map_wc(2);
    obj.insert("_version".to_string(), QueryValue::Int(version));
    obj.insert("_ts".to_string(), QueryValue::Null);
    let row = QueryRecord::Direct(QueryValue::Map(obj));

    let bytes = rmp_serde::to_vec_named(&row).expect("msgpack ser");
    let got: QueryValue = rmp_serde::from_slice(&bytes).expect("msgpack de");
    assert_eq!(got["_version"], QueryValue::Int(version));
    assert!(got["_ts"].is_null());
}
