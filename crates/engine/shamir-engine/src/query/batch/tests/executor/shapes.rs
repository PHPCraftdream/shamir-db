use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::select;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::filter::FilterValue;
use shamir_types::access::Actor;

use crate::query::auth::SessionPermissions;
use crate::query::batch::{
    execute_batch_unchecked as execute_batch, execute_batch_with_permissions, BatchError,
};

use super::common::setup_metrics_resolver;

fn aggregate_read(active: bool) -> Query {
    Query::from("users")
        .select([select::sum_of(FilterValue::field_ref("age"), "total")])
        .where_eq("active", active)
}

/// mutant: a recording path that never observes `total_nanos`, leaving it at
/// zero.
#[tokio::test]
async fn executions_errors_and_time_are_accounted_by_shape() {
    let resolver = setup_metrics_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.insert(
        "seed",
        write::insert("users")
            .row(doc().set("active", true).unwrap().set("age", 5).unwrap())
            .row(doc().set("active", true).unwrap().set("age", 6).unwrap()),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    resolver.metrics.shape_stats().reset();
    assert_eq!(resolver.metrics.shape_stats().tracked(), 0);

    let mut reads = Batch::new();
    reads.id(2);
    reads.query("on", aggregate_read(true));
    reads.query("off", aggregate_read(false));
    execute_batch(&reads.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let snapshot = resolver.metrics.shape_stats().snapshot();
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].1.executions, 2);
    assert_eq!(snapshot.rows[0].1.errors, 0);
    let key = snapshot.rows[0].0;
    let first_total = snapshot.rows[0].1.total_nanos;
    assert!(
        first_total > 0,
        "two executed reads must have accumulated shape time"
    );

    let mut malformed = Batch::new();
    malformed.id(3);
    malformed.insert(
        "malformed",
        write::insert("users").row(
            doc()
                .set("active", true)
                .unwrap()
                .set("age", "not a number")
                .unwrap(),
        ),
    );
    execute_batch(
        &malformed.build(),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .unwrap();

    let mut failing = Batch::new();
    failing.id(4);
    failing.query("on", aggregate_read(true));
    assert!(execute_batch(
        &failing.build(),
        &resolver,
        None,
        None,
        Actor::System,
        "test"
    )
    .await
    .is_err());

    let snapshot = resolver.metrics.shape_stats().snapshot();
    let row = snapshot
        .rows
        .iter()
        .find(|(row_key, _)| *row_key == key)
        .expect("the aggregate shape row must still be tracked");
    assert_eq!(row.1.executions, 3);
    assert_eq!(row.1.errors, 1);
    // the failing execution adds a non-negative elapsed; a zero-length execution is possible, so `>=` is the honest bound
    assert!(
        row.1.total_nanos >= first_total,
        "the failing execution adds a non-negative elapsed to {first_total}, got {}",
        row.1.total_nanos
    );
}

#[tokio::test]
async fn unknown_table_and_denied_read_never_create_shapes() {
    let resolver = setup_metrics_resolver().await;
    resolver.metrics.shape_stats().reset();

    let mut unknown = Batch::new();
    unknown.id(1);
    unknown.query("missing", Query::from("no_such_table"));
    let error = execute_batch(
        &unknown.build(),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .unwrap_err();
    assert!(matches!(error, BatchError::QueryError { .. }));
    assert_eq!(resolver.metrics.shape_stats().tracked(), 0);
    assert!(resolver.metrics.shape_stats().snapshot().rows.is_empty());

    let permissions = SessionPermissions::build(&[]);
    let mut denied = Batch::new();
    denied.id(2);
    denied.query("users", Query::from("users"));
    let error =
        execute_batch_with_permissions(&denied.build(), &resolver, None, &permissions, "test")
            .await
            .unwrap_err();
    assert!(matches!(error, BatchError::QueryError { .. }));
    assert_eq!(resolver.metrics.shape_stats().tracked(), 0);
    assert!(resolver.metrics.shape_stats().snapshot().rows.is_empty());
}
