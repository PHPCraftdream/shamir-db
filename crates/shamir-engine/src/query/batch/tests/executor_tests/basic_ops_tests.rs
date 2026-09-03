//! Basic single-op and multi-op batch execution tests.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::execute_batch_unchecked as execute_batch;

use super::common::setup_resolver;

// ============================================================================
// Single read query
// ============================================================================

#[tokio::test]
async fn test_single_read_query() {
    let resolver = setup_resolver().await;

    // Insert some data first
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "insert",
        write::insert("users")
            .row(doc().set("name", "Alice").set("age", 30))
            .row(doc().set("name", "Bob").set("age", 25)),
    );
    let insert_req = b.build();
    let resp = execute_batch(&insert_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["insert"].records.len(), 2);

    // Now read
    let mut b = Batch::new();
    b.id(1);
    b.query("users", Query::from("users"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results["users"].records.len(), 2);
    assert!(!resp.execution_plan.is_empty());
}

// ============================================================================
// Read query honors builder-supplied pagination (canonical wire path)
// ============================================================================
//
// F-group-5 (#1078 task group 5): the crate used to carry a second,
// hand-written `query_from_value` parser (deleted) that read pagination from
// an untagged top-level `"limit"` key instead of the tagged `pagination`
// object this canonical path (`BatchOp::Read` -> `qv_to::<ReadQuery>`)
// produces. Fed a builder-shaped query, that dead parser silently dropped
// `.limit(n)` and returned the whole table. This test pins the canonical
// path's actual (correct) behavior so that invariant has real coverage.

#[tokio::test]
async fn test_read_query_limit_bounds_results() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    let mut insert = write::insert("users");
    for i in 0..5 {
        insert = insert.row(doc().set("name", format!("user{i}")));
    }
    b.insert("insert", insert);
    let insert_req = b.build();
    let resp = execute_batch(&insert_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["insert"].records.len(), 5);

    let mut b = Batch::new();
    b.id(1);
    b.query("read", Query::from("users").limit(2));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // A `.limit(2)` query over 5 rows must return exactly 2 — not the
    // whole table.
    assert_eq!(resp.results["read"].records.len(), 2);
}

// ============================================================================
// Independent queries run in same stage
// ============================================================================

#[tokio::test]
async fn test_independent_queries_same_stage() {
    let resolver = setup_resolver().await;

    // Seed data
    let mut b = Batch::new();
    b.id(1);
    b.op_silent("s1", write::insert("users").row(doc().set("name", "Alice")));
    b.op_silent("s2", write::insert("orders").row(doc().set("item", "Book")));
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Two independent reads
    let mut b = Batch::new();
    b.id(1);
    b.query("users", Query::from("users"));
    b.query("orders", Query::from("orders"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Both in same stage (no dependencies)
    assert_eq!(resp.execution_plan.len(), 1);
    assert_eq!(resp.execution_plan[0].len(), 2);
    assert_eq!(resp.results.len(), 2);
}

// ============================================================================
// Insert + read pipeline
// ============================================================================

#[tokio::test]
async fn test_insert_then_read() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "insert",
        write::insert("users")
            .row(doc().set("name", "Alice").set("score", 100))
            .row(doc().set("name", "Bob").set("score", 50)),
    );
    b.query("read", Query::from("users"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Both in same stage (no explicit dependency)
    assert_eq!(resp.results["insert"].records.len(), 2);
    // Read may or may not see the inserted records depending on execution order
    // within the stage (sequential currently, so insert runs first)
    assert_eq!(resp.results["read"].records.len(), 2);
}

// ============================================================================
// return_only filtering
// ============================================================================

#[tokio::test]
async fn test_return_only() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "insert",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.query("read", Query::from("users"));
    b.return_only(["read"]);
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Only "read" returned
    assert_eq!(resp.results.len(), 1);
    assert!(resp.results.contains_key("read"));
}

// ============================================================================
// return_result = false
// ============================================================================

#[tokio::test]
async fn test_return_result_false() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.return_flagged();
    b.op_silent(
        "setup",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.query("read", Query::from("users"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // "setup" has return_result=false, "read" has return_result=true (default)
    assert_eq!(resp.results.len(), 1);
    assert!(resp.results.contains_key("read"));
}

// ============================================================================
// Delete in batch
// ============================================================================

#[tokio::test]
async fn test_batch_with_delete() {
    let resolver = setup_resolver().await;

    // Seed
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete inactive, then read
    let mut b = Batch::new();
    b.id(1);
    b.try_delete(
        "cleanup",
        write::delete("users").where_(shamir_query_builder::filter::eq("status", "inactive")),
    )
    .unwrap();
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    // 1 record deleted (Bob)
    assert_eq!(
        resp.results["cleanup"]
            .stats
            .as_ref()
            .unwrap()
            .records_scanned,
        1
    );
}
