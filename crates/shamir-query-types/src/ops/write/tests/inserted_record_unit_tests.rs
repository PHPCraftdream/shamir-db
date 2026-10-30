//! Unit tests for `InsertedRecord` (moved out of `ops/write/inserted_record.rs`).

use shamir_types::mpack;
use shamir_types::types::common::TMap;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{QueryValue, Value};

use crate::write::InsertedRecord;

fn make_direct() -> InsertedRecord {
    let mut map: TMap<String, Value<String>> = TMap::default();
    map.insert("name".to_string(), Value::Str("widget".to_string()));
    map.insert("qty".to_string(), Value::Int(42));
    let id = RecordId::system("test-id-00");
    InsertedRecord {
        id: Some(id),
        fields: QueryValue::Map(map),
    }
}

/// Wire-byte round-trip: a `Direct` record serialized to msgpack must
/// deserialize back to an equivalent `InsertedRecord`.
#[test]
fn inserted_record_msgpack_round_trip() {
    let direct = make_direct();
    let bytes = rmp_serde::to_vec_named(&direct).expect("serialize");
    let back: InsertedRecord = rmp_serde::from_slice(&bytes).expect("deserialize");
    // After round-trip, _id is stored in fields (the deserializer has no
    // special _id extraction; it lands in the QueryValue::Map). Verify
    // the data fields are present.
    let name = back.fields.get("name").and_then(QueryValue::as_str);
    let qty = back.fields.get("qty").and_then(QueryValue::as_i64);
    assert_eq!(name, Some("widget"));
    assert_eq!(qty, Some(42));
}

#[test]
fn inserted_record_get_value_owned_id() {
    let direct = make_direct();
    let id_val = direct.get_value_owned("_id");
    assert!(id_val.is_some(), "_id must be synthesised from id field");
    assert!(matches!(id_val, Some(QueryValue::Str(_))));
}

#[test]
fn inserted_record_get_value_owned_field() {
    let direct = make_direct();
    let qty = direct.get_value_owned("qty");
    assert_eq!(qty, Some(QueryValue::Int(42)));
}

/// Serialization must emit sorted keys (deterministic wire order).
#[test]
fn inserted_record_sorted_key_order() {
    let direct = make_direct();
    let bytes = rmp_serde::to_vec_named(&direct).expect("serialize");
    // Build the expected value via mpack! and serialize it.
    // Expected sorted order for fields {name, qty} + _id: _id, name, qty.
    let id_str = direct.id.as_ref().unwrap().to_string();
    let expected =
        mpack!({ "_id": @ QueryValue::Str(id_str.clone()), "name": "widget", "qty": 42_i64 });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(
        bytes, expected_bytes,
        "wire bytes must be sorted-key-order identical"
    );
}

#[test]
fn idless_non_map_values_round_trip() {
    for fields in [
        QueryValue::Null,
        QueryValue::Int(42),
        QueryValue::Bin(vec![1, 2, 3]),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Bool(true)]),
    ] {
        let original = InsertedRecord { id: None, fields };
        let bytes = rmp_serde::to_vec_named(&original).unwrap();
        let decoded: InsertedRecord = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, original);
    }
}

#[test]
fn inserted_record_no_id_serialization() {
    let rec = InsertedRecord {
        id: None,
        fields: mpack!({ "name": "widget", "qty": 42_i64 }),
    };
    let bytes = rmp_serde::to_vec_named(&rec).expect("serialize");
    let expected = mpack!({ "name": "widget", "qty": 42_i64 });
    let expected_bytes = rmp_serde::to_vec_named(&expected).expect("expected serialize");
    assert_eq!(bytes, expected_bytes);
}
