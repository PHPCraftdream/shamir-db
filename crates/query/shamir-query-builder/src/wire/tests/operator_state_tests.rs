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
use shamir_query_types::wire::{DbRequest, DbResponse, OperatorStatus};
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
