//! Query-language v4 typed request options: wire round-trip + strictness.
//!
//! Covers the `BatchRequest` v4 contract: typed `durability` /
//! `index_visibility` / `isolation` enums round-trip, unknown enum strings
//! are rejected, unknown top-level keys are rejected
//! (`#[serde(deny_unknown_fields)]`), and the removed
//! `durability: "async_index"` string no longer decodes.

use crate::batch::{
    BatchLimits, BatchRequest, IndexVisibility, IsolationLevel, RequestDurability, ResultEncoding,
};
use crate::wire::CURRENT_QUERY_LANG_VERSION;
use shamir_collections::new_map;
use shamir_types::types::value::QueryValue;

fn encode(v: &impl serde::Serialize) -> Vec<u8> {
    rmp_serde::to_vec_named(v).expect("encode")
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>> {
    rmp_serde::from_slice(bytes).map_err(Into::into)
}

/// Minimal valid `BatchRequest` payload as a raw msgpack map (via
/// [`QueryValue`]) so tests can inject/tamper individual keys.
fn base_request_qv() -> QueryValue {
    let req = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: true,
        isolation: None,
        durability: None,
        index_visibility: None,
        visibility_wait: None,
        queries: new_map(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    let bytes = encode(&req);
    rmp_serde::from_slice(&bytes).expect("to raw map")
}

/// (a) v4 round trip: durability Synced + index_visibility Async +
/// isolation Serializable serialize and deserialize back equal.
#[test]
fn v4_typed_options_roundtrip() {
    let req = BatchRequest {
        id: QueryValue::Int(7),
        name: None,
        transactional: true,
        isolation: Some(IsolationLevel::Serializable),
        durability: Some(RequestDurability::Synced),
        index_visibility: Some(IndexVisibility::Async),
        visibility_wait: None,
        queries: new_map(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    let bytes = encode(&req);
    let back: BatchRequest = decode(&bytes).expect("decode");
    assert_eq!(req, back);
}

/// (b) An unknown durability string is rejected at deserialization.
#[test]
fn unknown_durability_string_is_rejected() {
    let mut qv = base_request_qv();
    if let QueryValue::Map(ref mut m) = qv {
        m.insert("durability".to_string(), QueryValue::from("walpay"));
    }
    let bytes = encode(&qv);
    assert!(
        decode::<BatchRequest>(&bytes).is_err(),
        "unknown durability value must not decode"
    );
}

/// (c) deny_unknown_fields: an unknown top-level key is rejected.
#[test]
fn unknown_top_level_key_is_rejected() {
    let mut qv = base_request_qv();
    if let QueryValue::Map(ref mut m) = qv {
        // Typo'd key — must be rejected, not silently ignored.
        m.insert("durabilty".to_string(), QueryValue::from("synced"));
    }
    let bytes = encode(&qv);
    assert!(
        decode::<BatchRequest>(&bytes).is_err(),
        "unknown BatchRequest key must not decode"
    );
}

/// (d) The old `durability: "async_index"` string is rejected in v4.
#[test]
fn legacy_async_index_durability_string_is_rejected() {
    let mut qv = base_request_qv();
    if let QueryValue::Map(ref mut m) = qv {
        m.insert("durability".to_string(), QueryValue::from("async_index"));
    }
    let bytes = encode(&qv);
    assert!(
        decode::<BatchRequest>(&bytes).is_err(),
        "removed durability async_index value must not decode"
    );
}

/// (e) `index_visibility: "async"` decodes to
/// [`IndexVisibility::Async`] — the type-level successor of the old
/// async_index semantics; `"sync"` decodes to [`IndexVisibility::Sync`].
#[test]
fn index_visibility_wire_names() {
    let mut qv = base_request_qv();
    if let QueryValue::Map(ref mut m) = qv {
        m.insert("index_visibility".to_string(), QueryValue::from("async"));
    }
    let bytes = encode(&qv);
    let req: BatchRequest = decode(&bytes).expect("decode");
    assert_eq!(req.index_visibility, Some(IndexVisibility::Async));

    // Round-trips to the same wire name.
    assert_eq!(IndexVisibility::Async.as_str(), "async");
    assert_eq!(IndexVisibility::Sync.as_str(), "sync");
}

/// (f) The wire constant is v4; enum wire names are stable.
#[test]
fn query_lang_version_is_four_and_names_are_stable() {
    assert_eq!(CURRENT_QUERY_LANG_VERSION, 4);
    assert_eq!(RequestDurability::Buffered.as_str(), "buffered");
    assert_eq!(RequestDurability::Synced.as_str(), "synced");
    assert_eq!(IsolationLevel::Snapshot.as_str(), "snapshot");
    assert_eq!(IsolationLevel::Serializable.as_str(), "serializable");
    assert_eq!(IsolationLevel::Pessimistic.as_str(), "pessimistic");
}

/// The legacy `"level3"` isolation alias is dropped: rejected in v4.
#[test]
fn legacy_level3_isolation_alias_is_rejected() {
    let mut qv = base_request_qv();
    if let QueryValue::Map(ref mut m) = qv {
        m.insert("isolation".to_string(), QueryValue::from("level3"));
    }
    let bytes = encode(&qv);
    assert!(
        decode::<BatchRequest>(&bytes).is_err(),
        "dropped level3 alias must not decode"
    );
}
