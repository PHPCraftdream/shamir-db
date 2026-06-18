//! Shared test helpers for DDL tests.

use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

/// Serialize a `BatchOp` to msgpack, deserialize it back and assert equality,
/// then return the decoded `QueryValue` for structural assertions.
pub(super) fn roundtrip(op: &BatchOp) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(op).expect("serialize");
    let back: BatchOp = rmp_serde::from_slice(&bytes).expect("deserialize");
    assert_eq!(&back, op, "round-trip mismatch");
    rmp_serde::from_slice(&bytes).expect("QueryValue decode")
}
