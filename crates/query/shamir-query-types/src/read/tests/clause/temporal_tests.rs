//! Serde round-trip tests for `Temporal` and `At` — every variant
//! and the partial-fields case for `History`.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::read::{At, OrderDirection, Temporal};

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

/// `At::Version` round-trips with the expected wire shape.
#[test]
fn at_version_round_trip() {
    let at = At::Version(42);
    let qv = to_qv(&at);
    assert_eq!(qv, mpack!({ "version": 42_i64 }));

    let back: At = from_qv(qv);
    assert_eq!(back, at);
}

/// `At::Timestamp` round-trips with the expected wire shape.
#[test]
fn at_timestamp_round_trip() {
    let at = At::Timestamp(1_700_000_000_000);
    let qv = to_qv(&at);
    assert_eq!(
        qv,
        mpack!({ "timestamp": @ QueryValue::Int(1_700_000_000_000_i64) })
    );

    let back: At = from_qv(qv);
    assert_eq!(back, at);
}

/// `Temporal::Latest` is the default and serializes as expected.
#[test]
fn temporal_latest_default() {
    let t = Temporal::default();
    assert!(matches!(t, Temporal::Latest));
    assert!(t.is_latest());
}

/// `Temporal::AsOf` round-trips.
#[test]
fn temporal_as_of_round_trip() {
    let t = Temporal::AsOf {
        at: At::Version(99),
    };
    let qv = to_qv(&t);
    assert_eq!(qv, mpack!({ "kind": "as_of", "at": { "version": 99_i64 } }));

    let back: Temporal = from_qv(qv);
    assert_eq!(back, t);
    assert!(!back.is_latest());
}

/// `Temporal::History` with all fields set round-trips.
#[test]
fn temporal_history_full_round_trip() {
    let t = Temporal::History {
        from: Some(At::Version(1)),
        to: Some(At::Timestamp(2000)),
        limit: Some(100),
        order: OrderDirection::Desc,
    };
    let qv = to_qv(&t);
    assert_eq!(
        qv,
        mpack!({
            "kind": "history",
            "from": { "version": 1_i64 },
            "to": { "timestamp": 2000_i64 },
            "limit": 100_i64,
            "order": "desc"
        })
    );

    let back: Temporal = from_qv(qv);
    assert_eq!(back, t);
}

/// `Temporal::History` with only `from` set (partial fields) round-trips
/// and omits the unset optional fields.
#[test]
fn temporal_history_partial_from_only() {
    let t = Temporal::History {
        from: Some(At::Version(5)),
        to: None,
        limit: None,
        order: OrderDirection::Asc,
    };
    let qv = to_qv(&t);
    // `to`, `limit` absent; `order` present because it's not skip-serialized.
    assert_eq!(
        qv,
        mpack!({
            "kind": "history",
            "from": { "version": 5_i64 },
            "order": "asc"
        })
    );

    let back: Temporal = from_qv(qv);
    assert_eq!(back, t);
}

/// `Temporal::History` with only `limit` set (partial fields) round-trips.
#[test]
fn temporal_history_partial_limit_only() {
    let t = Temporal::History {
        from: None,
        to: None,
        limit: Some(50),
        order: OrderDirection::Asc,
    };
    let qv = to_qv(&t);
    assert_eq!(
        qv,
        mpack!({
            "kind": "history",
            "limit": 50_i64,
            "order": "asc"
        })
    );

    let back: Temporal = from_qv(qv);
    assert_eq!(back, t);
}

/// `Temporal::History` deserializes from msgpack that omits `order` —
/// defaults to `Asc`.
#[test]
fn temporal_history_order_defaults_to_asc() {
    let qv = mpack!({
        "kind": "history",
        "from": { "timestamp": 100_i64 }
    });
    let t: Temporal = from_qv(qv);
    match t {
        Temporal::History {
            from,
            to,
            limit,
            order,
        } => {
            assert_eq!(from, Some(At::Timestamp(100)));
            assert_eq!(to, None);
            assert_eq!(limit, None);
            assert_eq!(order, OrderDirection::Asc);
        }
        other => panic!("expected History, got {other:?}"),
    }
}
