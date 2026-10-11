//! W7 partial lifecycle coverage; no real-time sleeps or synthetic timeouts.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::BatchRequest;

use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::tx_registry::{InteractiveTx, TxRegistryError};
use crate::version::CURRENT_QUERY_LANG_VERSION;

async fn fixture(lifetime: Duration) -> (ShamirDbHandler, Session, u64) {
    let session = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(true, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        0,
    );
    let db = ShamirDb::init_memory().await.expect("init memory database");
    let owner = Actor::User(principal64(session.user_id));
    db.create_db_as("app", owner.clone()).await;
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        owner.clone(),
    )
    .await
    .expect("add repo");
    let (tx, snapshot) = db
        .tx_begin_as(owner, "app", "main", "snapshot")
        .await
        .expect("begin real transaction");
    let handle = tx.tx_id.0;
    let handler = ShamirDbHandler::new(Arc::new(db));
    handler
        .tx_registry
        .register(
            handle,
            InteractiveTx::new(
                tx,
                snapshot,
                session.session_id,
                session.user_id,
                "app".into(),
                "main".into(),
                lifetime,
            ),
        )
        .expect("park transaction");
    (handler, session, handle)
}

fn read(table: &str) -> BatchRequest {
    let mut batch = Batch::new();
    batch.query("read", Query::from(table));
    batch.build()
}

async fn execute(
    handler: &ShamirDbHandler,
    session: &Session,
    handle: u64,
    batch: BatchRequest,
) -> DbResponse {
    handler
        .tx_execute(session, CURRENT_QUERY_LANG_VERSION, "app", handle, batch)
        .await
}

// Match the existing handler fixtures' roomy stack for deep engine async frames.
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

/// Inferred red before W7: the valid read executes despite a zero lifetime.
#[test]
fn zero_lifetime_rejects_execute_removes_handle_and_frees_session() {
    on_big_stack(|| async {
        let (handler, session, handle) = fixture(Duration::ZERO).await;
        match execute(&handler, &session, handle, read("items")).await {
            DbResponse::Error { code, .. } => assert_eq!(code, "tx_expired"),
            other => panic!("expected expired transaction, got {other:?}"),
        }
        assert!(matches!(
            handler.tx_registry.get_owned(handle, &session.session_id),
            Err(TxRegistryError::TxNotFound),
        ));
        match execute(&handler, &session, handle, read("items")).await {
            DbResponse::Error { code, .. } => assert_eq!(code, "tx_not_found"),
            other => panic!("removed handle remained executable: {other:?}"),
        }
        let next = match handler
            .tx_begin(&session, CURRENT_QUERY_LANG_VERSION, "app", "main", None)
            .await
        {
            DbResponse::TxOpened { tx_handle, .. } => tx_handle,
            other => panic!("session slot not freed: {other:?}"),
        };
        let response = handler.tx_rollback(&session, "app", next).await;
        assert!(
            matches!(response, DbResponse::TxRolledBack { .. }),
            "{response:?}",
        );
    });
}

/// Preservation control: catches an over-broad change that drops every engine error.
#[test]
fn ordinary_query_error_preserves_handle_for_successful_execute() {
    on_big_stack(|| async {
        let (handler, session, handle) = fixture(Duration::from_secs(300)).await;
        match execute(&handler, &session, handle, read("missing")).await {
            DbResponse::Error { code, message } => {
                assert_eq!(code, "query", "{message}");
                assert!(message.contains("missing"), "{message}");
            }
            other => panic!("expected missing-table query error, got {other:?}"),
        }
        match execute(&handler, &session, handle, read("items")).await {
            DbResponse::TxBatch { response } => {
                let result = response.results.get("read").expect("executed read result");
                assert!(!result.skipped, "read must actually execute");
                assert!(result.records.is_empty(), "fixture table is empty");
                assert!(response.transaction.is_none(), "execute must not commit");
            }
            other => panic!("handle not usable after ordinary query error: {other:?}"),
        }
        let response = handler.tx_rollback(&session, "app", handle).await;
        assert!(
            matches!(response, DbResponse::TxRolledBack { .. }),
            "{response:?}",
        );
        assert!(handler.tx_registry.is_empty());
    });
}
