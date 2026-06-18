//! Backward-compat test for the §0 invariant: a `ReadQuery` msgpack payload with
//! NO `temporal`/`with_version` fields deserializes to `Latest`/`false`
//! and re-serializes WITHOUT adding those keys.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::read::{ReadQuery, Temporal};

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

/// A pre-temporal ReadQuery payload round-trips without adding
/// `temporal` or `with_version` keys.
#[test]
fn read_query_without_temporal_fields_is_unchanged() {
    let old_qv = mpack!({ "from": "users" });

    let parsed: ReadQuery = from_qv(old_qv);
    assert_eq!(parsed.temporal, Temporal::Latest);
    assert!(!parsed.with_version);

    // Re-serialize — the temporal/with_version keys must NOT appear.
    let re_serialized = to_qv(&parsed);
    assert!(
        re_serialized.get("temporal").is_none(),
        "expected 'temporal' absent in re-serialized payload, got: {re_serialized:?}"
    );
    assert!(
        re_serialized.get("with_version").is_none(),
        "expected 'with_version' absent in re-serialized payload, got: {re_serialized:?}"
    );
}

/// When `temporal` is explicitly `Latest` and `with_version` is `false`,
/// the serialized output still omits both keys (skip_serializing_if).
#[test]
fn read_query_explicit_latest_and_false_with_version_omitted() {
    let rq = ReadQuery::new("orders");
    let qv = to_qv(&rq);
    assert!(qv.get("temporal").is_none());
    assert!(qv.get("with_version").is_none());
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

    let qv = to_qv(&rq);
    assert_eq!(
        qv.get("with_version").and_then(QueryValue::as_bool),
        Some(true)
    );
    let temporal = qv.get("temporal").expect("temporal key present");
    assert_eq!(
        temporal.get("kind").and_then(QueryValue::as_str),
        Some("as_of")
    );
    let at = temporal.get("at").expect("at key present");
    assert_eq!(at.get("version").and_then(QueryValue::as_i64), Some(7));

    // Round-trip back.
    let back: ReadQuery = from_qv(qv);
    assert_eq!(back, rq);
}
