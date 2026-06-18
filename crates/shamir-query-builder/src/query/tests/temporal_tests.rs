//! Tests for the temporal builder methods on [`Query`].
//!
//! Each test asserts the exact wire DTO produced and (where applicable)
//! confirms that serialised msgpack omits keys that should remain off the
//! wire when default values are used.

use shamir_query_types::read::{At, OrderDirection, ReadQuery, Temporal};
use shamir_types::mpack;

use crate::query::Query;

// ── helpers ────────────────────────────────────────────────────────

fn wire(rq: &ReadQuery) -> shamir_types::types::value::QueryValue {
    let bytes = rmp_serde::to_vec_named(rq).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("decode QueryValue")
}

// ── as_of_version ─────────────────────────────────────────────────

#[test]
fn builder_as_of_version_sets_temporal() {
    let rq = Query::from("logs").as_of_version(5).build();

    assert_eq!(
        rq.temporal,
        Temporal::AsOf { at: At::Version(5) },
        "temporal should be AsOf {{ at: Version(5) }}"
    );

    // Wire: temporal key present with the expected shape.
    let qv = wire(&rq);
    assert_eq!(
        qv["temporal"],
        mpack!({
            "kind": "as_of",
            "at": {"version": 5}
        }),
    );
}

// ── as_of_timestamp ───────────────────────────────────────────────

#[test]
fn builder_as_of_timestamp_sets_temporal() {
    let ts: u64 = 1_700_000_000_000;
    let rq = Query::from("logs").as_of_timestamp(ts).build();

    assert_eq!(
        rq.temporal,
        Temporal::AsOf {
            at: At::Timestamp(ts)
        },
        "temporal should be AsOf {{ at: Timestamp({ts}) }}"
    );

    let qv = wire(&rq);
    assert_eq!(
        qv["temporal"],
        mpack!({
            "kind": "as_of",
            "at": {"timestamp": 1700000000000i64}
        }),
    );
}

// ── history (full timeline) ────────────────────────────────────────

#[test]
fn builder_history_sets_temporal() {
    let rq = Query::from("events").history().build();

    assert_eq!(
        rq.temporal,
        Temporal::History {
            from: None,
            to: None,
            limit: None,
            order: OrderDirection::Asc,
        },
        "history() should set History with all-None bounds and Asc order"
    );

    // Wire: from/to/limit are omitted (skip_serializing_if = None),
    // order defaults to "asc".
    let qv = wire(&rq);
    assert_eq!(
        qv["temporal"],
        mpack!({
            "kind": "history",
            "order": "asc"
        }),
    );
}

// ── history_range (bounded) ───────────────────────────────────────

#[test]
fn builder_history_range_sets_temporal() {
    let rq = Query::from("events")
        .history_range(
            Some(At::Version(10)),
            Some(At::Version(50)),
            Some(100),
            OrderDirection::Desc,
        )
        .build();

    assert_eq!(
        rq.temporal,
        Temporal::History {
            from: Some(At::Version(10)),
            to: Some(At::Version(50)),
            limit: Some(100),
            order: OrderDirection::Desc,
        },
        "history_range should set all provided fields"
    );

    let qv = wire(&rq);
    assert_eq!(
        qv["temporal"],
        mpack!({
            "kind": "history",
            "from": {"version": 10},
            "to":   {"version": 50},
            "limit": 100,
            "order": "desc"
        }),
    );
}

// ── with_version ───────────────────────────────────────────────────

#[test]
fn builder_with_version_sets_flag() {
    let rq = Query::from("docs").with_version().build();

    assert!(rq.with_version, "with_version should be true");

    // Wire: with_version key present.
    let qv = wire(&rq);
    assert_eq!(qv["with_version"], true);
}

// ── default (no temporal call) is Latest ─────────────────────────

#[test]
fn builder_without_temporal_is_latest() {
    let rq = Query::from("users").where_eq("active", true).build();

    // DTO: temporal == Latest, with_version == false.
    assert_eq!(rq.temporal, Temporal::Latest);
    assert!(!rq.with_version);

    // Wire: temporal and with_version keys are absent (skip_serialized).
    let qv = wire(&rq);
    assert!(
        qv.get("temporal").is_none(),
        "temporal key must be absent from wire for Latest"
    );
    assert!(
        qv.get("with_version").is_none(),
        "with_version key must be absent from wire when false"
    );

    // Round-trip: deserialising without those keys reproduces the same DTO.
    let bytes = rmp_serde::to_vec_named(&rq).expect("serialize");
    let back: ReadQuery = rmp_serde::from_slice(&bytes).expect("deserialize");
    assert_eq!(back, rq, "round-trip must be identical");
}
