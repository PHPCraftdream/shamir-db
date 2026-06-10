//! Tests for the `QueryRunner` struct (tx: None path).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::QueryRunner;

use super::common::setup_resolver;

// ============================================================================
// QueryRunner struct — tx: None path
// ============================================================================

#[tokio::test]
async fn test_query_runner_none_tx_insert_and_read() {
    let resolver = setup_resolver().await;

    // Insert via QueryRunner with tx: None
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("users").row(doc().set("name", "Eve").set("age", 28)),
    );
    let insert_req = b.build();
    let insert_entry = insert_req.queries.get("ins").unwrap().clone();
    let empty_params = shamir_types::types::common::new_map();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        invoker: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &empty_params,
    };
    let result = runner
        .run(
            "ins",
            &insert_entry,
            &shamir_types::types::common::new_map(),
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);

    // Read via QueryRunner with tx: None
    let mut b = Batch::new();
    b.id(2);
    b.query("q", Query::from("users"));
    let read_req = b.build();
    let read_entry = read_req.queries.get("q").unwrap().clone();
    let empty_params2 = shamir_types::types::common::new_map();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        invoker: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &empty_params2,
    };
    let result = runner
        .run("q", &read_entry, &shamir_types::types::common::new_map())
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
}
