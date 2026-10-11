//! W4 behavioral regressions. Red before wiring: only three legacy caps
//! reached execution; these requests succeeded despite the operator policy.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest};
use shamir_query_types::filter::FilterValue;

use crate::config::QueryLimitsConfig;
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

// Deep nested engine futures need the same roomy stack as W7 fixtures.
fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

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

async fn handler(cap: QueryLimitsCap) -> ShamirDbHandler {
    let db = ShamirDb::init_memory().await.expect("init");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone()).await;
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        owner,
    )
    .await
    .expect("repo");
    ShamirDbHandler::new(Arc::new(db)).with_query_limits(cap)
}

fn read() -> BatchRequest {
    let mut batch = Batch::new();
    batch.limits(BatchLimits::CEILING);
    batch.query("r", Query::from("items"));
    batch.build()
}

fn nested(depth: usize) -> BatchRequest {
    let mut request = read();
    for _ in 0..depth {
        let mut batch = Batch::new();
        batch.limits(BatchLimits {
            max_nesting_depth: 1000,
            ..BatchLimits::CEILING
        });
        batch.sub_batch_no_bind("nested", request);
        request = batch.build();
    }
    request
}

fn repeated() -> BatchRequest {
    let mut batch = Batch::new();
    batch.limits(BatchLimits::CEILING);
    batch.for_each(
        "loop",
        FilterValue::Array(vec![1.into(), 2.into(), 3.into()]),
        "row",
        read(),
    );
    batch.build()
}

async fn run(handler: &ShamirDbHandler, request: BatchRequest, tx: bool) -> DbResponse {
    let session = session();
    if tx {
        let opened = handler
            .tx_begin(&session, CURRENT_QUERY_LANG_VERSION, "app", "main", None)
            .await;
        let handle = match opened {
            DbResponse::TxOpened { tx_handle, .. } => tx_handle,
            other => panic!("begin: {other:?}"),
        };
        let response = handler
            .tx_execute(&session, CURRENT_QUERY_LANG_VERSION, "app", handle, request)
            .await;
        // Explicit cleanup also covers errors that intentionally leave the tx open.
        handler.tx_registry().remove(handle);
        response
    } else {
        handler
            .execute(
                &session,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                request,
                &ConnectionServices::without_push(0),
            )
            .await
    }
}

#[test]
fn server_policy_rejects_nesting_in_execute_and_tx_execute() {
    on_big_stack(server_policy_rejects_nesting_body);
}

#[test]
fn default_config_keeps_client_raised_limits_accepted() {
    on_big_stack(default_config_keeps_client_raised_limits_body);
}

#[test]
fn server_policy_rejects_iterations_in_execute_and_tx_execute() {
    on_big_stack(server_policy_rejects_iterations_body);
}

#[test]
fn server_policy_rejects_pagination_in_execute_and_tx_execute() {
    on_big_stack(server_policy_rejects_pagination_body);
}

#[test]
fn server_policy_rejects_aggregate_work_in_execute_and_tx_execute() {
    on_big_stack(server_policy_rejects_aggregate_work_body);
}

#[test]
fn server_policy_rejects_total_queries_in_execute_and_tx_execute() {
    on_big_stack(server_policy_rejects_total_queries_body);
}

fn assert_code(response: DbResponse, expected: &str) {
    match response {
        DbResponse::Error { code, .. } => assert_eq!(code, expected),
        other => panic!("expected {expected}, got {other:?}"),
    }
}

async fn server_policy_rejects_nesting_body() {
    let handler = handler(QueryLimitsCap {
        max_nesting_depth: 1,
        ..QueryLimitsCap::UNLIMITED
    })
    .await;
    for tx in [false, true] {
        assert_code(run(&handler, nested(2), tx).await, "nesting_too_deep");
    }
}

async fn server_policy_rejects_iterations_body() {
    let handler = handler(QueryLimitsCap {
        max_iterations: 2,
        ..QueryLimitsCap::UNLIMITED
    })
    .await;
    for tx in [false, true] {
        assert_code(run(&handler, repeated(), tx).await, "too_many_iterations");
    }
}

async fn server_policy_rejects_pagination_body() {
    let handler = handler(QueryLimitsCap {
        max_page_size: 2,
        max_offset: 0,
        ..QueryLimitsCap::UNLIMITED
    })
    .await;
    for tx in [false, true] {
        for query in [
            Query::from("items").limit(3),
            Query::from("items").offset(1),
        ] {
            let mut batch = Batch::new();
            batch.limits(BatchLimits::CEILING);
            batch.query("r", query);
            assert_code(
                run(&handler, batch.build(), tx).await,
                "pagination_too_large",
            );
        }
    }
}

async fn server_policy_rejects_aggregate_work_body() {
    // One loop + three body executions = four work units; only two AST nodes.
    let handler = handler(QueryLimitsCap {
        max_total_work: 3,
        ..QueryLimitsCap::UNLIMITED
    })
    .await;
    for tx in [false, true] {
        assert_code(run(&handler, repeated(), tx).await, "work_budget_exceeded");
    }
}

async fn server_policy_rejects_total_queries_body() {
    let handler = handler(QueryLimitsCap {
        max_total_queries: 1,
        ..QueryLimitsCap::UNLIMITED
    })
    .await;
    for tx in [false, true] {
        assert_code(run(&handler, nested(1), tx).await, "work_budget_exceeded");
    }
}

async fn default_config_keeps_client_raised_limits_body() {
    let handler = handler(QueryLimitsCap::from(&QueryLimitsConfig::default())).await;
    // Depth five exceeds the REQUEST default four, but was already accepted
    // when the client raised its limit. Operator defaults must not narrow it.
    for tx in [false, true] {
        let mut pagination = Batch::new();
        pagination.limits(BatchLimits::CEILING);
        pagination.query("r", Query::from("items").limit(100_000).offset(1_000_000));
        let mut iterations = Batch::new();
        iterations.limits(BatchLimits::CEILING);
        let mut body = Batch::new();
        body.limits(BatchLimits::CEILING);
        iterations.for_each(
            "loop",
            FilterValue::Array(vec![1.into(); 1001]),
            "row",
            body.build(),
        );
        for request in [
            read(),
            nested(5),
            repeated(),
            pagination.build(),
            iterations.build(),
        ] {
            let response = run(&handler, request, tx).await;
            match response {
                DbResponse::Batch { response } | DbResponse::TxBatch { response } => {
                    assert_eq!(response.results.len(), 1);
                }
                other => panic!("default config rejected existing request: {other:?}"),
            }
        }
    }
}
