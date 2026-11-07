//! Serde-default regression tests for [`BatchLimits`] (#662).
//!
//! The wire format is MessagePack (`rmp_serde`), not JSON — these tests
//! build the map literal via [`mpack!`] and round-trip it through
//! `rmp_serde::to_vec_named` / `from_slice`, exactly like sibling
//! `batch/tests/*.rs` files, so the assertion actually proves the real wire
//! behavior a client (e.g. the TS SDK) exercises.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::batch_limits::BatchLimits;

fn to_batch_limits(qv: &QueryValue) -> BatchLimits {
    let bytes = rmp_serde::to_vec_named(qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

/// A `limits` payload that omits `max_iterations` (the shape every client
/// built before #653 — and the TS client until #662 — sends on the wire)
/// must still deserialize successfully, filling `max_iterations` with the
/// same `1000` default `BatchLimits::default()` uses. Before the
/// `#[serde(default = "default_max_iterations")]` fix, this failed with
/// `"missing field \`max_iterations\`"`.
#[test]
fn missing_max_iterations_deserializes_with_default() {
    let qv = mpack!({
        "max_queries": 20,
        "max_dependency_depth": 5,
        "max_execution_time_secs": 10,
        "max_result_size": 1000000,
        "max_nesting_depth": 4
    });

    let limits = to_batch_limits(&qv);

    assert_eq!(limits.max_iterations, 1000);
    assert_eq!(limits.max_queries, 20);
    assert_eq!(limits.max_dependency_depth, 5);
    assert_eq!(limits.max_execution_time_secs, 10);
    assert_eq!(limits.max_result_size, 1_000_000);
    assert_eq!(limits.max_nesting_depth, 4);
}

/// A `limits` payload that DOES provide `max_iterations` must round-trip
/// the explicit value rather than silently falling back to the default.
#[test]
fn explicit_max_iterations_is_honoured() {
    let qv = mpack!({
        "max_queries": 20,
        "max_dependency_depth": 5,
        "max_execution_time_secs": 10,
        "max_result_size": 1000000,
        "max_nesting_depth": 4,
        "max_iterations": 42
    });

    let limits = to_batch_limits(&qv);

    assert_eq!(limits.max_iterations, 42);
}

/// `BatchLimits::default()`'s `max_iterations` value must match the serde
/// default used when the field is missing from the wire — otherwise a
/// client-omitted field and a programmatic `BatchLimits::default()` would
/// silently diverge.
#[test]
fn default_matches_serde_default() {
    let qv = mpack!({
        "max_queries": 50,
        "max_dependency_depth": 10,
        "max_execution_time_secs": 30,
        "max_result_size": 10485760,
        "max_nesting_depth": 4
    });

    let deserialized = to_batch_limits(&qv);
    assert_eq!(deserialized, BatchLimits::default());
}

/// RV-QUERY-TYPES-4BAE89B5: a time-only `limits` map (or any partial map of
/// the ORIGINAL five fields) must deserialize — every field carries a
/// `#[serde(default = ...)]` now, not just the post-#662 ones.
#[test]
fn rv_query_types_4bae89b5_time_only_limits_map_accepted() {
    let qv = mpack!({ "max_execution_time_secs": 10 });
    let limits = to_batch_limits(&qv);
    assert_eq!(limits.max_execution_time_secs, 10);
    assert_eq!(
        limits,
        BatchLimits {
            max_execution_time_secs: 10,
            ..BatchLimits::default()
        }
    );
}

#[test]
fn unknown_limit_is_rejected_with_its_name() {
    let value = serde_json::json!({
        "max_queris": 10
    });
    let error = serde_json::from_value::<BatchLimits>(value).unwrap_err();
    assert!(error.to_string().contains("max_queris"));
}
