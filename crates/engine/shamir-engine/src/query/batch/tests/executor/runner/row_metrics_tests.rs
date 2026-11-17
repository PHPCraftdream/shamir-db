//! End-to-end row counter coverage through the batch resolver boundary.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::select;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use super::super::common::setup_metrics_resolver;
use crate::query::batch::execute_batch_unchecked as execute_batch;

#[tokio::test]
async fn rows_viewed_and_returned_follow_actual_read_work() {
    let resolver = setup_metrics_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.insert(
        "insert",
        write::insert("users")
            .row(doc().set("active", true).unwrap().set("age", 10).unwrap())
            .row(doc().set("active", true).unwrap().set("age", 20).unwrap())
            .row(doc().set("active", false).unwrap().set("age", 30).unwrap()),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut reads = Batch::new();
    reads.id(2);
    reads.query(
        "page",
        Query::from("users")
            .where_eq("active", true)
            .count_total(true)
            .limit(1),
    );
    reads.query(
        "count",
        Query::from("users").select([select::count_all("n")]),
    );
    let response = execute_batch(&reads.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(response.results["page"].records.len(), 1);
    assert_eq!(response.results["count"].records.len(), 1);
    let snapshot = resolver.metrics.snapshot();
    // The count_total scan examines all three stored rows despite its one-row
    // page; the count(*) shortcut consults metadata and examines no table rows.
    assert_eq!(snapshot.rows_viewed, 3);
    assert_eq!(snapshot.rows_returned, 2);
}
