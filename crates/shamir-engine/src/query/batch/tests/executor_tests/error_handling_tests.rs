//! Error handling and validation tests for the batch executor.

use serde_json::json;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::execute_batch;

use super::common::setup_resolver;

// ============================================================================
// Circular dependency error
// ============================================================================

#[tokio::test]
async fn test_circular_dependency_error() {
    let resolver = setup_resolver().await;

    // a depends on b, b depends on a
    let mut b = Batch::new();
    b.id(1);
    // We need to build $query refs manually because Handle is returned per-insert
    // and we'd need both aliases before either is registered.
    // Use raw qref since it's a circular dep that can't be expressed via Handle.
    b.query(
        "a",
        Query::from("users").where_eq("id", shamir_query_builder::val::qref("b", "[0].id")),
    );
    b.query(
        "b",
        Query::from("users").where_eq("id", shamir_query_builder::val::qref("a", "[0].id")),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::CircularDependency { .. }
    ));
}

// ============================================================================
// Pre-validation: unknown table fails before execution
// ============================================================================

#[tokio::test]
async fn test_unknown_table_fails_early() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "good",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.query("bad", Query::from("nonexistent_table"));
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap_err();
    // Should fail with table not found error BEFORE any execution
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Request ID echoed in response
// ============================================================================

#[tokio::test]
async fn test_request_id_echoed() {
    let resolver = setup_resolver().await;

    // String ID
    let mut b = Batch::new();
    b.id("req-42");
    b.query("q", Query::from("users"));
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.id, json!("req-42"));

    // Numeric ID
    let mut b = Batch::new();
    b.id(123);
    b.query("q", Query::from("users"));
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.id, json!(123));
}
