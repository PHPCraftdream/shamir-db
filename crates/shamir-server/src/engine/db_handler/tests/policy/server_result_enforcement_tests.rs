//! W4b: real handler execution, not merely the policy declaration.
//! Red before wiring: the cumulative silent reads succeed and commit the marker.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_query_types::batch::{BatchLimits, BatchRequest, BatchResponse};

use crate::config::QueryLimitsConfig;
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

const CAP: usize = 7_000;

fn session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        0,
    )
}

async fn execute(handler: &ShamirDbHandler, request: BatchRequest) -> DbResponse {
    handler
        .execute(
            &session(),
            CURRENT_QUERY_LANG_VERSION,
            "app",
            request,
            &ConnectionServices::without_push(0),
        )
        .await
}

fn batch_response(response: DbResponse) -> BatchResponse {
    match response {
        DbResponse::Batch { response } => response,
        other => panic!("expected batch response, got {other:?}"),
    }
}

async fn fixture(enabled: bool) -> (ShamirDbHandler, ShamirDbHandler) {
    let db = ShamirDb::init_memory().await.expect("init");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("items"))
            .add_table(TableConfig::new("markers")),
        owner,
    )
    .await
    .expect("repo");
    let db = Arc::new(db);
    let observer =
        ShamirDbHandler::new(Arc::clone(&db)).with_query_limits(QueryLimitsCap::UNLIMITED);
    let mut seed = Batch::new();
    seed.insert(
        "seed",
        insert("items").row(doc! { "payload" => "x".repeat(4096) }),
    );
    let seeded = batch_response(execute(&observer, seed.build()).await);
    assert_eq!(seeded.results["seed"].records.len(), 1);

    // Exercise the complete operator config -> cap -> runtime policy chain.
    let config = QueryLimitsConfig {
        enforce_result_size: enabled,
        max_result_size_bytes: CAP,
        ..QueryLimitsConfig::default()
    };
    let handler = ShamirDbHandler::new(db).with_query_limits(QueryLimitsCap::from(&config));
    (handler, observer)
}

fn materializing_batch(reads: usize) -> BatchRequest {
    let mut batch = Batch::new();
    batch.transactional();
    batch.return_flagged();
    batch.limits(BatchLimits::CEILING);
    let mut previous = batch.insert("marker", insert("markers").row(doc! { "name" => "staged" }));
    for index in 0..reads {
        let read = batch.query_silent(format!("hidden_{index}"), Query::from("items"));
        batch.after(&read, &previous);
        previous = read;
    }
    batch.build()
}

async fn marker_count(observer: &ShamirDbHandler) -> usize {
    let mut batch = Batch::new();
    batch.query("markers", Query::from("markers"));
    batch_response(execute(observer, batch.build()).await).results["markers"]
        .records
        .len()
}

fn assert_committed_marker(response: DbResponse) {
    let response = batch_response(response);
    assert!(response
        .transaction
        .expect("transaction outcome")
        .is_committed());
    assert_eq!(
        response.results.len(),
        1,
        "silent intermediates stay hidden",
    );
    assert_eq!(response.results["marker"].records.len(), 1);
}

#[tokio::test]
async fn server_result_enforcement_cumulative_silent_reads_abort_before_commit() {
    let (handler, observer) = fixture(true).await;
    // Each 4096-byte string plus record/result shells is individually below
    // 7000 bytes; two materializations exceed it. Neither read is returned.
    // The preceding write must roll back, not commit then fail serialization.
    let request = materializing_batch(2);
    assert!(!request.queries["hidden_0"].return_result);
    assert!(!request.queries["hidden_1"].return_result);
    match execute(&handler, request).await {
        DbResponse::Error { code, .. } => assert_eq!(code, "result_too_large"),
        other => panic!("expected mid-execution byte-budget error, got {other:?}"),
    }
    assert_eq!(
        marker_count(&observer).await,
        0,
        "no staged write may commit",
    );
}

#[tokio::test]
async fn server_result_enforcement_below_cap_commits_and_filters_intermediate() {
    let (handler, observer) = fixture(true).await;
    assert_committed_marker(execute(&handler, materializing_batch(1)).await);
    assert_eq!(marker_count(&observer).await, 1);
}

#[tokio::test]
async fn server_result_enforcement_false_preserves_over_cap_batch() {
    let (handler, observer) = fixture(false).await;
    assert_committed_marker(execute(&handler, materializing_batch(2)).await);
    assert_eq!(marker_count(&observer).await, 1);
}
