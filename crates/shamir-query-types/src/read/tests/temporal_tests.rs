//! Serde round-trip tests for `Temporal` and `At` — every variant
//! and the partial-fields case for `History`.

use serde_json::json;

use crate::read::{At, OrderDirection, Temporal};

/// `At::Version` round-trips with the expected wire shape.
#[test]
fn at_version_round_trip() {
    let at = At::Version(42);
    let json_val = serde_json::to_value(&at).expect("serialize");
    assert_eq!(json_val, json!({ "version": 42 }));

    let back: At = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(back, at);
}

/// `At::Timestamp` round-trips with the expected wire shape.
#[test]
fn at_timestamp_round_trip() {
    let at = At::Timestamp(1_700_000_000_000);
    let json_val = serde_json::to_value(&at).expect("serialize");
    assert_eq!(json_val, json!({ "timestamp": 1700000000000u64 }));

    let back: At = serde_json::from_value(json_val).expect("deserialize");
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
    let json_val = serde_json::to_value(&t).expect("serialize");
    assert_eq!(
        json_val,
        json!({ "kind": "as_of", "at": { "version": 99 } })
    );

    let back: Temporal = serde_json::from_value(json_val).expect("deserialize");
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
    let json_val = serde_json::to_value(&t).expect("serialize");
    assert_eq!(
        json_val,
        json!({
            "kind": "history",
            "from": { "version": 1 },
            "to": { "timestamp": 2000 },
            "limit": 100,
            "order": "desc"
        })
    );

    let back: Temporal = serde_json::from_value(json_val).expect("deserialize");
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
    let json_val = serde_json::to_value(&t).expect("serialize");
    // `to`, `limit` absent; `order` present because it's not skip-serialized.
    assert_eq!(
        json_val,
        json!({
            "kind": "history",
            "from": { "version": 5 },
            "order": "asc"
        })
    );

    let back: Temporal = serde_json::from_value(json_val).expect("deserialize");
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
    let json_val = serde_json::to_value(&t).expect("serialize");
    assert_eq!(
        json_val,
        json!({
            "kind": "history",
            "limit": 50,
            "order": "asc"
        })
    );

    let back: Temporal = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(back, t);
}

/// `Temporal::History` deserializes from JSON that omits `order` —
/// defaults to `Asc`.
#[test]
fn temporal_history_order_defaults_to_asc() {
    let json_val = json!({
        "kind": "history",
        "from": { "timestamp": 100 }
    });
    let t: Temporal = serde_json::from_value(json_val).expect("deserialize");
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
