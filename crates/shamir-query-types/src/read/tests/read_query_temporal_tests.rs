//! Backward-compat test for the §0 invariant: a `ReadQuery` JSON with
//! NO `temporal`/`with_version` fields deserializes to `Latest`/`false`
//! and re-serializes WITHOUT adding those keys.

use serde_json::{json, Value};

use crate::read::{ReadQuery, Temporal};

/// A pre-temporal ReadQuery JSON shape round-trips byte-identically:
/// deserializes to `temporal == Latest`, `with_version == false`, and
/// re-serializes WITHOUT `temporal` or `with_version` keys.
#[test]
fn read_query_without_temporal_fields_is_unchanged() {
    let old_json = json!({
        "from": "users"
    });

    let parsed: ReadQuery = serde_json::from_value(old_json).expect("deserialize");
    assert_eq!(parsed.temporal, Temporal::Latest);
    assert!(!parsed.with_version);

    // Re-serialize — the temporal/with_version keys must NOT appear.
    let re_serialized: Value = serde_json::to_value(&parsed).expect("serialize");
    assert!(
        re_serialized.get("temporal").is_none(),
        "expected 'temporal' absent in re-serialized JSON, got: {re_serialized}"
    );
    assert!(
        re_serialized.get("with_version").is_none(),
        "expected 'with_version' absent in re-serialized JSON, got: {re_serialized}"
    );
}

/// When `temporal` is explicitly `Latest` and `with_version` is `false`,
/// the serialized output still omits both keys (skip_serializing_if).
#[test]
fn read_query_explicit_latest_and_false_with_version_omitted() {
    let rq = ReadQuery::new("orders");
    let json_val = serde_json::to_value(&rq).expect("serialize");
    assert!(json_val.get("temporal").is_none());
    assert!(json_val.get("with_version").is_none());
}

/// A ReadQuery with `with_version: true` and a non-Latest temporal
/// serializes both keys onto the wire.
#[test]
fn read_query_with_version_and_as_of_serialized() {
    let mut rq = ReadQuery::new("events");
    rq.with_version = true;
    rq.temporal = Temporal::AsOf {
        at: crate::read::At::Version(7),
    };

    let json_val = serde_json::to_value(&rq).expect("serialize");
    assert_eq!(json_val["with_version"], json!(true));
    assert_eq!(
        json_val["temporal"],
        json!({ "kind": "as_of", "at": { "version": 7 } })
    );

    // Round-trip back.
    let back: ReadQuery = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(back, rq);
}
