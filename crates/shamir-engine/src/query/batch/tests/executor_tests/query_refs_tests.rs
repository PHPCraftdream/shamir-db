//! Tests for $query reference dependencies between batch ops.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::execute_batch;

use super::common::setup_resolver;

// ============================================================================
// Dependent queries: $query ref
// ============================================================================

#[tokio::test]
async fn test_dependent_query_ref() {
    let resolver = setup_resolver().await;

    // Seed users
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive"))
            .row(doc().set("name", "Carol").set("status", "active")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Query 1: get active users
    // Query 2: get users where name == first active user's name (via $query ref)
    let mut b = Batch::new();
    b.id(1);
    let active = b.query("active", Query::from("users").where_eq("status", "active"));
    b.query(
        "first_active",
        Query::from("users").where_eq("name", active.first().field("name")),
    );
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Two stages: [active], [first_active]
    assert_eq!(resp.execution_plan.len(), 2);
    assert_eq!(resp.results["active"].records.len(), 2); // Alice + Carol
    assert_eq!(resp.results["first_active"].records.len(), 1); // Alice
}
