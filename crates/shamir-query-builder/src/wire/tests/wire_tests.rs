//! Tests for the `wire` module — JSON + msgpack multi-format encoding.

use serde_json::json;
use shamir_query_types::batch::BatchRequest;

use crate::batch::Batch;
use crate::wire::ToWire;
use crate::Query;

// ============================================================================
// Batch convenience methods
// ============================================================================

#[test]
fn batch_to_json_value_matches_build() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    let from_convenience = batch.to_json_value().unwrap();
    let from_build = serde_json::to_value(batch.build()).unwrap();

    assert_eq!(from_convenience, from_build);
}

#[test]
fn batch_to_json_value_matches_expected() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    let val = batch.to_json_value().unwrap();

    // Verify structural keys
    assert!(val.get("queries").is_some());
    let queries = val["queries"].as_object().unwrap();
    assert!(queries.contains_key("u"));

    // The query targets "users" (TableRef serializes as just the table string
    // when repo == "main", or as an object — check either form).
    let u_entry = &queries["u"];
    let from = &u_entry["from"];
    // TableRef serializes as a string "users" (flat) or {"table":"users","repo":"main"}
    let targets_users = from == "users" || from == &json!({"table": "users", "repo": "main"});
    assert!(targets_users, "unexpected `from`: {from:?}");
}

#[test]
fn batch_to_json_string_roundtrips() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users").where_eq("active", true));

    let json_str = batch.to_json_string().unwrap();
    let parsed: BatchRequest = serde_json::from_str(&json_str).unwrap();

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
fn read_query_to_json_value() {
    let rq = Query::from("tasks").where_eq("done", false).build();
    let val = rq.to_json_value().unwrap();

    let from = &val["from"];
    let targets_tasks = from == "tasks" || from == &json!({"table": "tasks", "repo": "main"});
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
// Pretty JSON (smoke test — just make sure it doesn't panic)
// ============================================================================

#[test]
fn batch_to_json_string_pretty_contains_newlines() {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users"));

    let pretty = batch.to_json_string_pretty().unwrap();
    assert!(pretty.contains('\n'));
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
