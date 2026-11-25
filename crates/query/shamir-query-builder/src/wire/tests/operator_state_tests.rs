//! Roundtrip and wire-boundary tests for the operator-state builders
//! (admin-shell R5 PR3: `DbRequest::Status` / `DbRequest::GetRecord`
//! and the `DbResponse::Status` / `DbResponse::Record` replies).
//!
//! Fixtures are built independently through the typed DTOs (allowed
//! for serde roundtrip tests); no raw query assembly anywhere. The
//! malformed-wire boundary test proves the fixed 16-byte `RecordId`
//! codec refuses a short key as a typed decode error, never a
//! successful decode of a wrong-sized id.

use shamir_query_builder::wire::admin::{get_record, status};
use shamir_query_types::wire::{
    DbRequest, DbResponse, OperatorStatus, StatusRole, StatusStampOutcome,
};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

fn test_key() -> RecordId {
    RecordId([0xAB; 16])
}

#[test]
fn status_builds_the_typed_request() {
    assert!(matches!(status(), DbRequest::Status));
}

#[test]
fn status_roundtrips_named_msgpack() {
    let request = status();
    let bytes = rmp_serde::to_vec_named(&request).expect("status encodes");
    let decoded: DbRequest = rmp_serde::from_slice(&bytes).expect("status decodes");
    assert!(matches!(decoded, DbRequest::Status), "got {decoded:?}");
}

#[test]
fn status_roundtrips_json() {
    let request = status();
    let text = serde_json::to_string(&request).expect("status encodes as JSON");
    assert!(text.contains("\"status\""), "discriminator in {text}");
    let decoded: DbRequest = serde_json::from_str(&text).expect("status decodes");
    assert!(matches!(decoded, DbRequest::Status), "got {decoded:?}");
}

#[test]
fn get_record_builds_the_typed_request_fields() {
    let request = get_record("main", "analytics", "events", test_key());
    match request {
        DbRequest::GetRecord {
            db,
            repo,
            table,
            key,
        } => {
            assert_eq!(db, "main");
            assert_eq!(repo, "analytics");
            assert_eq!(table, "events");
            assert_eq!(key, test_key());
        }
        other => panic!("expected GetRecord, got {other:?}"),
    }
}

#[test]
fn get_record_roundtrips_named_msgpack() {
    let request = get_record("main", "analytics", "events", test_key());
    let bytes = rmp_serde::to_vec_named(&request).expect("get_record encodes");
    let decoded: DbRequest = rmp_serde::from_slice(&bytes).expect("get_record decodes");
    // `DbRequest` has no `PartialEq`: compare the canonical re-encode.
    let re_encoded = rmp_serde::to_vec_named(&decoded).expect("decoded re-encodes");
    assert_eq!(re_encoded, bytes, "round trip preserves the request");
}

#[test]
fn get_record_roundtrips_json() {
    let request = get_record("main", "analytics", "events", test_key());
    let text = serde_json::to_string(&request).expect("get_record encodes as JSON");
    let decoded: DbRequest = serde_json::from_str(&text).expect("get_record decodes");
    let re_encoded = serde_json::to_string(&decoded).expect("decoded re-encodes");
    assert_eq!(re_encoded, text, "round trip preserves the request");
}

/// The fixed-size wire boundary: the key must be EXACTLY 16 msgpack
/// `bin` bytes. A 15-byte key is a typed decode error, never a
/// silently re-sized id. Built by patching the encoded form of a valid
/// request (whose key bytes are known `0xAB` repeats), so the only
/// difference is the key's declared length.
#[test]
fn get_record_short_key_is_a_typed_decode_error() {
    let request = get_record("main", "analytics", "events", test_key());
    let bytes = rmp_serde::to_vec_named(&request).expect("get_record encodes");

    // Locate the key: bin8 header (0xC4, 0x10) followed by 16 x 0xAB.
    let start = bytes
        .windows(3)
        .position(|w| w == [0xC4, 0x10, 0xAB])
        .expect("bin8-encoded 16-byte key present in the named msgpack form");

    // Rewrite the key as bin8 of 15 bytes: header len 0x10 -> 0x0F and
    // drop the last key byte.
    let mut malformed = bytes.clone();
    malformed[start + 1] = 0x0F;
    malformed.remove(start + 2 + 15);

    let decoded: Result<DbRequest, _> = rmp_serde::from_slice(&malformed);
    assert!(
        decoded.is_err(),
        "a 15-byte key must be a decode error, got {decoded:?}"
    );
}

/// Typed reply-side fixture for [`OperatorStatus`] — built through the
/// DTO's own `Deserialize` (independent typed fixture, allowed here);
/// vectors stay empty: presence and roundtrip are what is under test,
/// not the server's snapshot contents.
fn status_fixture() -> OperatorStatus {
    serde_json::from_value(serde_json::json!({
        "server_version": "0.1.0-test",
        "format_epoch": 4,
        "stamp_outcome": "unchanged",
        "uptime_ms": 12345,
        "role": "follower",
        "node_id": "node-7",
        "replication": [],
        "repositories": []
    }))
    .expect("typed OperatorStatus fixture decodes")
}

#[test]
fn status_reply_roundtrips_named_msgpack() {
    let response = DbResponse::Status {
        status: status_fixture(),
    };
    let bytes = rmp_serde::to_vec_named(&response).expect("reply encodes");
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).expect("reply decodes");
    // `DbResponse` has no `PartialEq`: compare the canonical re-encode.
    let re_encoded = rmp_serde::to_vec_named(&decoded).expect("decoded re-encodes");
    assert_eq!(re_encoded, bytes, "round trip preserves the reply");
}

#[test]
fn status_reply_roundtrips_json() {
    let response = DbResponse::Status {
        status: status_fixture(),
    };
    let text = serde_json::to_string(&response).expect("reply encodes as JSON");
    let decoded: DbResponse = serde_json::from_str(&text).expect("reply decodes");
    let re_encoded = serde_json::to_string(&decoded).expect("decoded re-encodes");
    assert_eq!(re_encoded, text, "round trip preserves the reply");
}

#[test]
fn record_reply_roundtrips_named_msgpack() {
    let response = DbResponse::Record {
        record: shamir_query_types::read::QueryRecord::Direct(QueryValue::Null),
    };
    let bytes = rmp_serde::to_vec_named(&response).expect("reply encodes");
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).expect("reply decodes");
    let re_encoded = rmp_serde::to_vec_named(&decoded).expect("decoded re-encodes");
    assert_eq!(re_encoded, bytes, "round trip preserves the reply");
}

#[test]
fn record_reply_roundtrips_json() {
    let response = DbResponse::Record {
        record: shamir_query_types::read::QueryRecord::Direct(QueryValue::Null),
    };
    let text = serde_json::to_string(&response).expect("reply encodes as JSON");
    let decoded: DbResponse = serde_json::from_str(&text).expect("reply decodes");
    let re_encoded = serde_json::to_string(&decoded).expect("decoded re-encodes");
    assert_eq!(re_encoded, text, "round trip preserves the reply");
}

// ── Hand-written msgpack frames ──────────────────────────────────────
//
// The roundtrip tests above encode and decode through the SAME codec, so a
// field renamed on both sides still passes. These frames are written BY
// HAND from the wire contract in `shamir_query_types::wire::db_message` and
// are the subject under test: a rename of `op` / `kind` / `db` / `repo` /
// `table` / `key` / `record` / `status` (or of any `OperatorStatus` field)
// makes them undecodable.

// Wire-format test: hand-written msgpack is the subject under test (the builder-only rule covers query construction, not codec shape).

/// `{"op": "get_record", "db": "main", "repo": "analytics", "table":
/// "events", "key": bin16(0xAB x16)}` — msgpack fixmap(5).
const GET_RECORD_FRAME: &[u8] = &[
    0x85, // map(5)
    0xA2, b'o', b'p', // "op" (fixstr 2)
    0xAA, b'g', b'e', b't', b'_', b'r', b'e', b'c', b'o', b'r', b'd', // "get_record"
    0xA2, b'd', b'b', // "db"
    0xA4, b'm', b'a', b'i', b'n', // "main"
    0xA4, b'r', b'e', b'p', b'o', // "repo"
    0xA9, b'a', b'n', b'a', b'l', b'y', b't', b'i', b'c', b's', // "analytics"
    0xA5, b't', b'a', b'b', b'l', b'e', // "table"
    0xA6, b'e', b'v', b'e', b'n', b't', b's', // "events"
    0xA3, b'k', b'e', b'y', // "key"
    0xC4, 0x10, // bin8, 16 bytes
    0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, // 8
    0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, // 16
];

/// `{"op": "status"}` — msgpack fixmap(1).
const STATUS_FRAME: &[u8] = &[
    0x81, // map(1)
    0xA2, b'o', b'p', // "op"
    0xA6, b's', b't', b'a', b't', b'u', b's', // "status"
];

/// `{"kind": "record", "record": {"n": 7}}` — the `record` field is the
/// bare projected record value (`QueryRecord::Direct` serializes as its
/// `QueryValue`).
const RECORD_REPLY_FRAME: &[u8] = &[
    0x82, // map(2)
    0xA4, b'k', b'i', b'n', b'd', // "kind"
    0xA6, b'r', b'e', b'c', b'o', b'r', b'd', // "record"
    0xA6, b'r', b'e', b'c', b'o', b'r', b'd', // "record"
    0x81, 0xA1, b'n', 0x07, // {"n": 7}
];

/// `{"kind": "status", "status": {...}}` — a MINIMAL `OperatorStatus` with
/// every documented field: `server_version`, `format_epoch`,
/// `stamp_outcome`, `uptime_ms`, `role`, `node_id`, `replication`,
/// `repositories`.
const STATUS_REPLY_FRAME: &[u8] = &[
    0x82, // map(2)
    0xA4, b'k', b'i', b'n', b'd', // "kind"
    0xA6, b's', b't', b'a', b't', b'u', b's', // "status"
    0xA6, b's', b't', b'a', b't', b'u', b's', // "status"
    0x88, // map(8)
    0xAE, // "server_version" (fixstr 14)
    b's', b'e', b'r', b'v', b'e', b'r', b'_', b'v', b'e', b'r', b's', b'i', b'o',
    b'n', // 14 bytes
    0xAA, b'0', b'.', b'1', b'.', b'0', b'-', b'w', b'i', b'r', b'e', // "0.1.0-wire"
    0xAC, b'f', b'o', b'r', b'm', b'a', b't', b'_', b'e', b'p', b'o', b'c',
    b'h', // "format_epoch"
    0x04, // 4
    0xAD, // "stamp_outcome" (fixstr 13)
    b's', b't', b'a', b'm', b'p', b'_', b'o', b'u', b't', b'c', b'o', b'm', b'e', // 13 bytes
    0xA9, b'u', b'n', b'c', b'h', b'a', b'n', b'g', b'e', b'd', // "unchanged"
    0xA9, b'u', b'p', b't', b'i', b'm', b'e', b'_', b'm', b's', // "uptime_ms"
    0x0C, // 12
    0xA4, b'r', b'o', b'l', b'e', // "role"
    0xA8, b'f', b'o', b'l', b'l', b'o', b'w', b'e', b'r', // "follower"
    0xA7, b'n', b'o', b'd', b'e', b'_', b'i', b'd', // "node_id"
    0xC0, // nil
    0xAB, b'r', b'e', b'p', b'l', b'i', b'c', b'a', b't', b'i', b'o', b'n', // "replication"
    0x90, // []
    0xAC, b'r', b'e', b'p', b'o', b's', b'i', b't', b'o', b'r', b'i', b'e',
    b's', // "repositories"
    0x90, // []
];

#[test]
fn get_record_decodes_from_hand_written_msgpack() {
    let decoded: DbRequest =
        rmp_serde::from_slice(GET_RECORD_FRAME).expect("hand-written get_record frame decodes");
    match decoded {
        DbRequest::GetRecord {
            db,
            repo,
            table,
            key,
        } => {
            assert_eq!(db, "main");
            assert_eq!(repo, "analytics");
            assert_eq!(table, "events");
            assert_eq!(key, test_key(), "the key is a fixed 16-byte msgpack bin");
        }
        other => panic!("expected GetRecord, got {other:?}"),
    }
}

#[test]
fn status_decodes_from_hand_written_msgpack() {
    let decoded: DbRequest =
        rmp_serde::from_slice(STATUS_FRAME).expect("hand-written status frame decodes");
    assert!(matches!(decoded, DbRequest::Status), "got {decoded:?}");
}

#[test]
fn record_reply_decodes_from_hand_written_msgpack() {
    let decoded: DbResponse =
        rmp_serde::from_slice(RECORD_REPLY_FRAME).expect("hand-written record reply decodes");
    let DbResponse::Record { record } = decoded else {
        panic!("expected Record, got {decoded:?}");
    };
    let shamir_query_types::read::QueryRecord::Direct(value) = record else {
        panic!("expected a direct record, got {record:?}");
    };
    let QueryValue::Map(fields) = value else {
        panic!("expected a map record, got {value:?}");
    };
    assert_eq!(fields.get("n"), Some(&QueryValue::Int(7)), "got {fields:?}");
    assert_eq!(fields.len(), 1, "exactly the hand-written field");
}

#[test]
fn status_reply_decodes_from_hand_written_msgpack() {
    let decoded: DbResponse =
        rmp_serde::from_slice(STATUS_REPLY_FRAME).expect("hand-written status reply decodes");
    let DbResponse::Status { status } = decoded else {
        panic!("expected Status, got {decoded:?}");
    };
    assert_eq!(status.server_version, "0.1.0-wire");
    assert_eq!(status.format_epoch, 4);
    assert_eq!(status.stamp_outcome, StatusStampOutcome::Unchanged);
    assert_eq!(status.uptime_ms, 12);
    assert_eq!(status.role, StatusRole::Follower);
    assert!(status.node_id.is_none());
    assert!(status.replication.is_empty());
    assert!(status.repositories.is_empty());
}
