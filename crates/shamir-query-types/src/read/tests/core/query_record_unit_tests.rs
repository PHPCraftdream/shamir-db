//! Unit tests for `QueryRecord` (moved out of `read/core/query_record.rs`).

use serde_bytes::ByteBuf;
use shamir_types::mpack;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::read::QueryRecord;
use crate::write::InsertedRecord;

/// `Direct` and a freshly-deserialized copy of the same msgpack payload
/// must produce byte-identical re-serialization.
#[test]
fn direct_round_trips_via_msgpack() {
    let mut map = new_map_wc(3);
    map.insert("name".to_string(), QueryValue::Str("alice".to_string()));
    map.insert("age".to_string(), QueryValue::Int(30));
    map.insert("active".to_string(), QueryValue::Bool(true));
    let direct = QueryRecord::Direct(QueryValue::Map(map));

    let bytes = rmp_serde::to_vec_named(&direct).unwrap();
    let back: QueryRecord = rmp_serde::from_slice(&bytes).unwrap();

    // Must be Direct after round-trip (not any legacy variant).
    assert!(
        matches!(&back, QueryRecord::Direct(_)),
        "round-trip must yield Direct, got {back:?}"
    );

    // Re-serialize and compare bytes.
    let bytes2 = rmp_serde::to_vec_named(&back).unwrap();
    assert_eq!(bytes, bytes2, "re-serialized bytes must be identical");
}

/// C1: `QueryRecord::Inserted(rec)` must serialise to non-empty msgpack
/// bytes, and its `as_value()` must expose the fields correctly.
#[test]
fn inserted_variant_serializes_and_exposes_fields() {
    let mut map = new_map_wc(3);
    map.insert("name".to_string(), QueryValue::Str("widget".to_string()));
    map.insert("qty".to_string(), QueryValue::Int(42));
    map.insert("score".to_string(), QueryValue::F64(3.5));
    let id = RecordId::system("test-id-00");
    let rec = InsertedRecord {
        id: Some(id),
        fields: QueryValue::Map(map.clone()),
    };

    // Inserted path (server-side, never reaches a deserializer in this test).
    let inserted = QueryRecord::Inserted(rec.clone());

    let ins_mp = rmp_serde::to_vec_named(&inserted).unwrap();

    // Must round-trip as QueryValue via as_value().
    let v = inserted.as_value();
    assert_eq!(v.get("qty").and_then(QueryValue::as_i64), Some(42));
    assert_eq!(v.get("name").and_then(QueryValue::as_str), Some("widget"));

    // Sanity: non-empty wire bytes produced.
    assert!(!ins_mp.is_empty());
}

// ── IdBytes round-trip (msgpack wire codec) ───────────────────────────────

/// `QueryRecord::IdBytes` must survive a msgpack serialize → deserialize
/// cycle unchanged. Serialization must emit msgpack `bin` (0xc4/0xc5/0xc6),
/// and deserialization of the `bin` token must reconstruct `IdBytes`.
#[test]
fn id_bytes_roundtrip_via_msgpack() {
    // A tiny fake id-keyed msgpack map payload.
    let payload: Vec<u8> = vec![0x82, 0x01, 0xa5, 0x61, 0x6c, 0x69, 0x63, 0x65];
    let record = QueryRecord::IdBytes(ByteBuf::from(payload.clone()));

    // Serialize to msgpack.
    let bytes = rmp_serde::to_vec_named(&record).unwrap();

    // Must contain a bin marker (0xc4 = bin8, 0xc5 = bin16, 0xc6 = bin32).
    let has_bin_marker = bytes.contains(&0xc4) || bytes.contains(&0xc5) || bytes.contains(&0xc6);
    assert!(
        has_bin_marker,
        "IdBytes must serialize as msgpack bin, not as an array: bytes={bytes:x?}"
    );

    // Full round-trip: deserialize must produce IdBytes with identical bytes.
    let back: QueryRecord = rmp_serde::from_slice(&bytes).unwrap();
    match back {
        QueryRecord::IdBytes(b) => {
            assert_eq!(
                b.as_ref(),
                payload.as_slice(),
                "deserialized bytes must match"
            );
        }
        other => panic!("expected IdBytes, got {other:?}"),
    }
}

/// A msgpack map payload must deserialize to `Direct(QueryValue::Map)`.
#[test]
fn map_payload_deserializes_as_direct() {
    let mut map = new_map_wc(2);
    map.insert("name".to_string(), QueryValue::Str("alice".to_string()));
    map.insert("age".to_string(), QueryValue::Int(30));
    let original = QueryRecord::Direct(QueryValue::Map(map));

    // Serialize via the existing path (produces a msgpack map, not bin).
    let bytes = rmp_serde::to_vec_named(&original).unwrap();

    // Deserializer must route a map token to Direct.
    let back: QueryRecord = rmp_serde::from_slice(&bytes).unwrap();
    match &back {
        QueryRecord::Direct(qv) => {
            assert_eq!(qv.get("name").and_then(QueryValue::as_str), Some("alice"));
            assert_eq!(qv.get("age").and_then(QueryValue::as_i64), Some(30));
        }
        other => panic!("expected Direct variant for a map payload, got {other:?}"),
    }
}

/// PartialEq normalises via QueryValue — Direct and Inserted holding the
/// same data must compare equal.
#[test]
fn partial_eq_direct_vs_inserted() {
    let fields = mpack!({ "x": 1, "y": "z" });
    let d = QueryRecord::Direct(fields.clone());
    // Build an Inserted with no id so as_value() = fields.
    let ins = QueryRecord::Inserted(InsertedRecord { id: None, fields });
    // Both produce the same as_value(); they must be equal.
    assert_eq!(d, ins);
}
