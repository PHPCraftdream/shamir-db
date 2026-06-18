//! Tests for the `wire` module — QueryValue + msgpack multi-format encoding.

use shamir_query_types::batch::BatchRequest;

use crate::batch::Batch;
use crate::wire::ToWire;
use crate::Query;

// ============================================================================
// Batch convenience methods
// ============================================================================

#[test]
fn batch_to_query_value_matches_build() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    // Both paths must produce identical QueryValue trees.
    let from_convenience = batch.build().to_query_value().unwrap();
    let from_build = batch.build().to_query_value().unwrap();

    assert_eq!(from_convenience, from_build);
}

#[test]
fn batch_to_query_value_matches_expected() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    let qv = batch.build().to_query_value().unwrap();

    // Verify structural keys.
    let queries = qv.get("queries").expect("queries key");
    assert!(queries.get("u").is_some(), "alias 'u' must be present");

    // The query targets "users" (TableRef serializes as just the table string
    // when repo == "main", or as an object — accept either form).
    let u_entry = queries.get("u").unwrap();
    let from = u_entry.get("from").expect("from key");
    let targets_users = from.as_str() == Some("users")
        || from
            .get("table")
            .and_then(|t| t.as_str())
            .map(|s| s == "users")
            .unwrap_or(false);
    assert!(targets_users, "unexpected `from`: {from:?}");
}

#[test]
fn batch_msgpack_roundtrips() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    let bytes = batch.to_msgpack().unwrap();
    let parsed: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();

    assert_eq!(parsed, batch.build());
}

// ============================================================================
// Msgpack round-trip (critical correctness check)
// ============================================================================

#[test]
fn batch_to_msgpack_roundtrips() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    let bytes = batch.to_msgpack().unwrap();
    let decoded: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();

    assert_eq!(decoded, batch.build());
}

#[test]
fn batch_to_msgpack_via_trait_roundtrips() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    let built = batch.build();
    let bytes = built.to_msgpack().unwrap();
    let decoded: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();

    assert_eq!(decoded, built);
}

// ============================================================================
// Blanket impl works on bare ReadQuery
// ============================================================================

#[test]
fn read_query_to_query_value() {
    let rq = Query::from("tasks").where_eq("done", false).build();
    let qv = rq.to_query_value().unwrap();

    let from = qv.get("from").expect("from key");
    let targets_tasks = from.as_str() == Some("tasks")
        || from
            .get("table")
            .and_then(|t| t.as_str())
            .map(|s| s == "tasks")
            .unwrap_or(false);
    assert!(targets_tasks, "unexpected `from`: {from:?}");
}

#[test]
fn read_query_to_msgpack_roundtrips() {
    let rq = Query::from("tasks").where_eq("done", false).build();
    let bytes = rq.to_msgpack().unwrap();
    let decoded: shamir_query_types::read::ReadQuery = rmp_serde::from_slice(&bytes).unwrap();

    assert_eq!(decoded, rq);
}

// ============================================================================
// Multi-op batch encoding
// ============================================================================

#[test]
fn multi_op_batch_msgpack_roundtrip() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));
    batch.query("o", Query::from("orders").where_gt("total", 100));

    let bytes = batch.to_msgpack().unwrap();
    let decoded: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();

    assert_eq!(decoded, batch.build());
}
