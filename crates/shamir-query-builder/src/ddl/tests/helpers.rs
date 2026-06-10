//! Shared test helpers for DDL tests.

use shamir_query_types::batch::BatchOp;

/// Serialize a `BatchOp` to a `serde_json::Value`, then deserialize it
/// back and assert equality.
pub(super) fn roundtrip(op: &BatchOp) -> serde_json::Value {
    let val = serde_json::to_value(op).expect("serialize");
    let back: BatchOp = serde_json::from_value(val.clone()).expect("deserialize");
    assert_eq!(&back, op, "round-trip mismatch");
    val
}
