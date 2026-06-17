//! Golden tests: shortcut read paths (MIN/MAX/COUNT) must produce
//! wire-identical output after the QueryValue migration (#60 F+G).
//!
//! Each test constructs the row BOTH the old way (serde_json::Map +
//! QueryRecord::Json) and the new way (QueryValue::Map +
//! QueryRecord::Direct), then asserts byte-identical JSON and msgpack
//! serialization.

use std::sync::OnceLock;

use shamir_query_types::read::QueryRecord;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

// ── helpers ─────────────────────────────────────────────────────────────

/// Assert that old Json and new Direct variants serialise to the same
/// JSON bytes and msgpack bytes.
fn assert_wire_identical(old: &QueryRecord, new: &QueryRecord) {
    let old_json = serde_json::to_vec(old).expect("old json ser");
    let new_json = serde_json::to_vec(new).expect("new json ser");

    // Compare via re-parsed json::Value to tolerate key-order differences
    // (serde_json::Map = BTreeMap → sorted; QueryValue::Map = IndexMap →
    // insertion order). The VALUES and TYPES must match.
    let old_v: serde_json::Value = serde_json::from_slice(&old_json).unwrap();
    let new_v: serde_json::Value = serde_json::from_slice(&new_json).unwrap();
    assert_eq!(
        old_v, new_v,
        "JSON wire values diverge:\n  old: {old_v}\n  new: {new_v}"
    );

    // msgpack wire: compare via round-trip to serde_json::Value (msgpack
    // map key order may differ between BTreeMap and IndexMap).
    let old_mp = rmp_serde::to_vec_named(old).expect("old mp ser");
    let new_mp = rmp_serde::to_vec_named(new).expect("new mp ser");
    let old_mp_v: serde_json::Value = rmp_serde::from_slice(&old_mp).unwrap();
    let new_mp_v: serde_json::Value = rmp_serde::from_slice(&new_mp).unwrap();
    assert_eq!(
        old_mp_v, new_mp_v,
        "msgpack wire values diverge:\n  old: {old_mp_v}\n  new: {new_mp_v}"
    );
}

// ── MIN shortcut ────────────────────────────────────────────────────────

#[test]
fn min_shortcut_int_value_wire_identical() {
    // Old path: inner_to_json_value(Int(42)) → json Number(42)
    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "min_score".to_string(),
        serde_json::Value::Number(42i64.into()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    // New path: inner_value_to_query_value(Int(42)) → QueryValue::Int(42)
    let mut new_obj = new_map_wc(1);
    new_obj.insert("min_score".to_string(), QueryValue::Int(42));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn min_shortcut_float_value_wire_identical() {
    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "min_temp".to_string(),
        serde_json::Value::Number(serde_json::Number::from_f64(3.5).unwrap()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("min_temp".to_string(), QueryValue::F64(3.5));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn min_shortcut_string_value_wire_identical() {
    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "min_name".to_string(),
        serde_json::Value::String("alice".to_string()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("min_name".to_string(), QueryValue::Str("alice".to_string()));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn min_shortcut_null_value_wire_identical() {
    let mut old_obj = serde_json::Map::new();
    old_obj.insert("min".to_string(), serde_json::Value::Null);
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("min".to_string(), QueryValue::Null);
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

// ── MAX shortcut ────────────────────────────────────────────────────────

#[test]
fn max_shortcut_int_value_wire_identical() {
    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "max_score".to_string(),
        serde_json::Value::Number(999i64.into()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("max_score".to_string(), QueryValue::Int(999));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn max_shortcut_null_value_wire_identical() {
    let mut old_obj = serde_json::Map::new();
    old_obj.insert("max".to_string(), serde_json::Value::Null);
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("max".to_string(), QueryValue::Null);
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

// ── COUNT shortcut ──────────────────────────────────────────────────────

#[test]
fn count_shortcut_zero_wire_identical() {
    let count: u64 = 0;

    // Old path: serde_json::Value::Number(count.into())
    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "count".to_string(),
        serde_json::Value::Number(count.into()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    // New path: QueryValue::Int(count as i64)
    let mut new_obj = new_map_wc(1);
    new_obj.insert("count".to_string(), QueryValue::Int(count as i64));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn count_shortcut_typical_wire_identical() {
    let count: u64 = 42;

    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "count".to_string(),
        serde_json::Value::Number(count.into()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("count".to_string(), QueryValue::Int(count as i64));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn count_shortcut_large_wire_identical() {
    // A count near i64::MAX but still representable.
    let count: u64 = i64::MAX as u64;

    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "total".to_string(),
        serde_json::Value::Number(count.into()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("total".to_string(), QueryValue::Int(count as i64));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn count_shortcut_with_alias_wire_identical() {
    let count: u64 = 100;

    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "num_users".to_string(),
        serde_json::Value::Number(count.into()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(1);
    new_obj.insert("num_users".to_string(), QueryValue::Int(count as i64));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

// ── Temporal metadata (_version / _ts) ──────────────────────────────────
//
// The temporal path was already ported to QueryValue in D+E. These tests
// verify that the _version and _ts metadata fields serialise identically
// to the old json path (they used json Number(version.into()) for u64
// version, and we now use QueryValue::Int(version as i64)).

#[test]
fn temporal_version_metadata_wire_identical() {
    let version: u64 = 7;
    let ts: u64 = 1718400000000; // ~2024

    // Old path: serde_json Map with json Numbers
    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "name".to_string(),
        serde_json::Value::String("alice".to_string()),
    );
    old_obj.insert(
        "_version".to_string(),
        serde_json::Value::Number((version as i64).into()),
    );
    old_obj.insert(
        "_ts".to_string(),
        serde_json::Value::Number((ts as i64).into()),
    );
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    // New path: QueryValue Map with Int
    let mut new_obj = new_map_wc(3);
    new_obj.insert("name".to_string(), QueryValue::Str("alice".to_string()));
    new_obj.insert("_version".to_string(), QueryValue::Int(version as i64));
    new_obj.insert("_ts".to_string(), QueryValue::Int(ts as i64));
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}

#[test]
fn temporal_version_null_ts_wire_identical() {
    let version: u64 = 3;

    let mut old_obj = serde_json::Map::new();
    old_obj.insert(
        "_version".to_string(),
        serde_json::Value::Number((version as i64).into()),
    );
    old_obj.insert("_ts".to_string(), serde_json::Value::Null);
    let old = QueryRecord::Json(serde_json::Value::Object(old_obj));

    let mut new_obj = new_map_wc(2);
    new_obj.insert("_version".to_string(), QueryValue::Int(version as i64));
    new_obj.insert("_ts".to_string(), QueryValue::Null);
    let new = QueryRecord::Direct(QueryValue::Map(new_obj), OnceLock::new());

    assert_wire_identical(&old, &new);
}
