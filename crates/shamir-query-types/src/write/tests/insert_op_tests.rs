//! Round-trip tests for `InsertOp`, including the new `records_idmsgpack`
//! pass-through field.

use serde_bytes::ByteBuf;
use shamir_types::types::value::QueryValue;

use crate::write::InsertOp;
use crate::TableRef; // TableRef::new used in helper

// ── helpers ─────────────────────────────────────────────────────────────────

fn table_ref() -> TableRef {
    TableRef::new("users")
}

// ── test 1: old-payload (no records_idmsgpack) deserializes with default ────

/// An old client that knows nothing about `records_idmsgpack` sends a payload
/// without the field. Deserialization must succeed and the field must default
/// to an empty Vec — proving forward-compat for legacy senders.
#[test]
fn insert_op_old_payload_defaults_records_idmsgpack_to_empty() {
    // Old wire payload: only `insert_into` (as string — TableRef wire format)
    // + `values`, no `records_idmsgpack`.
    let old: InsertOp = rmp_serde::from_slice(
        &rmp_serde::to_vec_named(&serde_json::json!({
            "insert_into": "users",
            "values": []
        }))
        .unwrap(),
    )
    .unwrap();

    assert!(
        old.records_idmsgpack.is_empty(),
        "absent records_idmsgpack must default to empty Vec"
    );
}

// ── test 2: new InsertOp with records_idmsgpack round-trips via rmp_serde ───

/// A new-style `InsertOp` with `records_idmsgpack` populated must survive a
/// msgpack serialize → deserialize cycle and compare equal.
///
/// Also proves that `ByteBuf` serializes as msgpack `bin` (not seq-of-u8):
/// the `serde_bytes` attribute emits a single bin token per element, and
/// rmp_serde reassembles it back to an identical `ByteBuf`.
#[test]
fn insert_op_records_idmsgpack_roundtrip_via_msgpack() {
    let raw_record: Vec<u8> = vec![0x82, 0x01, 0xa5, 0x61, 0x6c, 0x69, 0x63, 0x65];
    let op = InsertOp {
        insert_into: table_ref(),
        values: vec![QueryValue::Null],
        records_idmsgpack: vec![ByteBuf::from(raw_record.clone())],
    };

    // Serialize to msgpack (named map format — the real transport shape).
    let bytes = rmp_serde::to_vec_named(&op).unwrap();

    // Verify the element is msgpack bin, NOT a seq-of-u8 (array of ints).
    // msgpack bin-8 format code is 0xc4.  If serde_bytes is working correctly,
    // the serialized form contains 0xc4 (bin8 marker) followed by the length
    // byte and the payload — not 0x98 (fixarray) / 0xdc (array-16).
    assert!(
        bytes.contains(&0xc4),
        "records_idmsgpack element must serialize as msgpack bin8 (0xc4), \
         not as a seq-of-u8 array: bytes={bytes:x?}"
    );

    // Full round-trip equality.
    let back: InsertOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op, back, "InsertOp must round-trip via msgpack");
    assert_eq!(
        back.records_idmsgpack[0].as_ref(),
        raw_record.as_slice(),
        "deserialized bytes must match original"
    );
}
