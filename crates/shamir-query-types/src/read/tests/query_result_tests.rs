//! Tests for `CorruptRecordRef`'s wire encoding (F-22, #815).
//!
//! `CorruptRecordRef.id` must serialize as a base58 STRING on the wire —
//! the same convention every other `RecordId` uses (see
//! `InsertedRecord`'s `_id`) — NOT raw msgpack bytes (which is what
//! `RecordId`'s own derived `Serialize` impl emits via
//! `serialize_bytes`).

use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::read::CorruptRecordRef;

fn make_ref() -> CorruptRecordRef {
    CorruptRecordRef {
        table: "widgets".to_string(),
        id: RecordId::system("test-corrupt"),
    }
}

/// Wire-byte round-trip: serialize→deserialize must yield the SAME `id`.
#[test]
fn corrupt_record_ref_msgpack_round_trip() {
    let original = make_ref();
    let bytes = rmp_serde::to_vec_named(&original).expect("serialize");
    let back: CorruptRecordRef = rmp_serde::from_slice(&bytes).expect("deserialize");
    assert_eq!(back, original);
}

/// Directly verifiable proof of wire shape: decoding the serialized bytes
/// generically (as `QueryValue`, which distinguishes msgpack `str` from
/// `bin` on deserialize — `Value::Str` vs `Value::Bin`) must show `id` as
/// a `Str` (base58 string), never a `Bin` (raw bytes) — the bug this task
/// fixes (`RecordId`'s own derived `Serialize` emits `bin` via
/// `serialize_bytes`).
#[test]
fn corrupt_record_ref_id_is_msgpack_string_not_bytes() {
    let original = make_ref();
    let bytes = rmp_serde::to_vec_named(&original).expect("serialize");

    let decoded: QueryValue = rmp_serde::from_slice(&bytes).expect("decode as QueryValue");
    let id_value = decoded.get("id").expect("map must contain an `id` key");

    assert!(
        matches!(id_value, QueryValue::Str(_)),
        "id must decode as QueryValue::Str (base58 string), got {id_value:?}"
    );
    assert_eq!(
        id_value.as_str(),
        Some(original.id.to_string().as_str()),
        "id string must be the RecordId's base58 form"
    );
    assert!(
        !matches!(id_value, QueryValue::Bin(_)),
        "id must NOT decode as QueryValue::Bin (raw bytes) — the F-22 bug being fixed"
    );
}
