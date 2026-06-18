//! Tests for `QueryRecord` QueryValue-native accessor methods.
//!
//! All test data is built with `mpack!` — no `serde_json::json!` or raw JSON strings.

use shamir_types::mpack;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::read::QueryRecord;
use crate::write::InsertedRecord;

// ── helpers ───────────────────────────────────────────────────────────────────

fn direct_row() -> QueryRecord {
    QueryRecord::Direct(mpack!({
        "name": "alice",
        "age":  30,
        "active": true,
        "score": 9.5
    }))
}

fn inserted_direct_row() -> QueryRecord {
    let fields = mpack!({
        "name": "widget",
        "qty":  42
    });
    let id = RecordId::system("test-id-qr");
    let rec = InsertedRecord::Direct {
        id: Some(id),
        fields,
    };
    QueryRecord::Inserted(rec)
}

fn json_row() -> QueryRecord {
    // Build via msgpack round-trip so we get a deserialized Direct variant.
    let qv = mpack!({ "name": "bob", "age": 25 });
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

// ── as_value ──────────────────────────────────────────────────────────────────

#[test]
fn as_value_direct_borrows() {
    let row = direct_row();
    let v = row.as_value();
    // Must be Borrowed — Direct path is zero-allocation.
    assert!(matches!(v, std::borrow::Cow::Borrowed(_)));
    // Map key present.
    assert_eq!(v.get("name").and_then(QueryValue::as_str), Some("alice"));
}

#[test]
fn as_value_deserialized_direct_borrows() {
    // After Stage C the deserializer produces Direct; as_value() borrows.
    let row = json_row();
    let v = row.as_value();
    // Direct path borrows — no allocation.
    assert!(matches!(v, std::borrow::Cow::Borrowed(_)));
    assert_eq!(v.get("name").and_then(QueryValue::as_str), Some("bob"));
    assert_eq!(v.get("age").and_then(QueryValue::as_i64), Some(25));
}

#[test]
fn as_value_inserted_direct_owned() {
    let row = inserted_direct_row();
    let v = row.as_value();
    assert!(matches!(v, std::borrow::Cow::Owned(_)));
    assert_eq!(v.get("qty").and_then(QueryValue::as_i64), Some(42));
}

#[test]
fn as_value_id_bytes_returns_null() {
    use serde_bytes::ByteBuf;
    let row = QueryRecord::IdBytes(ByteBuf::from(vec![0x01, 0x02]));
    let v = row.as_value();
    assert_eq!(*v, QueryValue::Null);
}

// ── get_value (Direct-only borrow) ───────────────────────────────────────────

#[test]
fn get_value_direct_present() {
    let row = direct_row();
    let v = row.get_value("name").expect("key must be present");
    assert_eq!(v.as_str(), Some("alice"));
}

#[test]
fn get_value_direct_absent_returns_none() {
    let row = direct_row();
    assert!(row.get_value("does_not_exist").is_none());
}

#[test]
fn get_value_non_direct_returns_none() {
    // Only Direct supports get_value borrow; Inserted and IdBytes return None.
    use serde_bytes::ByteBuf;
    let ins_row = inserted_direct_row();
    assert!(ins_row.get_value("name").is_none());

    let id_bytes_row = QueryRecord::IdBytes(ByteBuf::from(vec![0x01]));
    assert!(id_bytes_row.get_value("name").is_none());
}

// ── get_value_owned ───────────────────────────────────────────────────────────

#[test]
fn get_value_owned_direct() {
    let row = direct_row();
    let v = row.get_value_owned("age").expect("key must be present");
    assert_eq!(v.as_i64(), Some(30));
}

#[test]
fn get_value_owned_deserialized() {
    // json_row() now produces a Direct via msgpack round-trip.
    let row = json_row();
    let v = row.get_value_owned("age").expect("age present in row");
    assert_eq!(v.as_i64(), Some(25));
}

#[test]
fn get_value_owned_inserted() {
    let row = inserted_direct_row();
    // Inserted::Direct carries only the fields (not _id), so "qty" should be present.
    let v = row
        .get_value_owned("qty")
        .expect("qty present in Inserted row");
    assert_eq!(v.as_i64(), Some(42));
}

#[test]
fn get_value_owned_absent_returns_none() {
    let row = direct_row();
    assert!(row.get_value_owned("nope").is_none());
}

// ── get_value_str ─────────────────────────────────────────────────────────────

#[test]
fn get_value_str_direct_present() {
    let row = direct_row();
    assert_eq!(row.get_value_str("name"), Some("alice"));
}

#[test]
fn get_value_str_direct_wrong_type() {
    let row = direct_row();
    // "age" is an Int, not a Str.
    assert!(row.get_value_str("age").is_none());
}

// ── get_value_i64 ─────────────────────────────────────────────────────────────

#[test]
fn get_value_i64_direct() {
    let row = direct_row();
    assert_eq!(row.get_value_i64("age"), Some(30));
}

#[test]
fn get_value_i64_wrong_type() {
    let row = direct_row();
    // "name" is a Str, not Int.
    assert!(row.get_value_i64("name").is_none());
}

// ── get_value_u64 ─────────────────────────────────────────────────────────────

#[test]
fn get_value_u64_direct() {
    let row = direct_row();
    // age = 30 ≥ 0, should fit in u64.
    assert_eq!(row.get_value_u64("age"), Some(30));
}

#[test]
fn get_value_u64_negative_returns_none() {
    let row = QueryRecord::Direct(mpack!({ "x": @(mpack!(-5)) }));
    assert!(row.get_value_u64("x").is_none());
}

// ── get_value_bool ────────────────────────────────────────────────────────────

#[test]
fn get_value_bool_direct() {
    let row = direct_row();
    assert_eq!(row.get_value_bool("active"), Some(true));
}

#[test]
fn get_value_bool_wrong_type() {
    let row = direct_row();
    // "age" is Int, not Bool.
    assert!(row.get_value_bool("age").is_none());
}

// ── QueryValue accessors on Value<String> ────────────────────────────────────

#[test]
fn query_value_as_i64_as_u64_as_bool_as_f64() {
    let v_int = mpack!(42);
    assert_eq!(v_int.as_i64(), Some(42));
    assert_eq!(v_int.as_u64(), Some(42));
    assert!(v_int.as_bool().is_none());

    let v_neg = mpack!(-1);
    assert_eq!(v_neg.as_i64(), Some(-1));
    assert!(v_neg.as_u64().is_none()); // negative → None

    let v_bool = mpack!(true);
    assert_eq!(v_bool.as_bool(), Some(true));
    assert!(v_bool.as_i64().is_none());

    let v_f = mpack!(9.5);
    assert!((v_f.as_f64().unwrap() - 9.5).abs() < 1e-10);
    assert!(v_f.as_i64().is_none());
}

#[test]
fn query_value_get_map_lookup() {
    let v = mpack!({ "x": 1, "y": 2 });
    assert_eq!(v.get("x").and_then(QueryValue::as_i64), Some(1));
    assert!(v.get("missing").is_none());

    // get on a non-map returns None.
    let v_str = mpack!("hello");
    assert!(v_str.get("any").is_none());
}

#[test]
fn query_value_is_map_is_list() {
    assert!(mpack!({}).is_map());
    assert!(!mpack!([]).is_map());
    assert!(mpack!([]).is_list());
    assert!(!mpack!({}).is_list());
}
