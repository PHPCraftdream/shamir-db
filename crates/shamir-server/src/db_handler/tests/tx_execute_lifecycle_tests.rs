//! W7 partial lifecycle coverage; no real-time sleeps or synthetic timeouts.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_query_builder::write::{doc, insert};
use shamir_query_types::batch::VisibilityWait;
use shamir_query_types::wire::DbRequest;

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

#[test]
fn vis1_legacy_requests_reject_before_writes_and_commit_consumption() {
    on_big_stack(|| async {
        let (handler, session, handle) = fixture(Duration::from_secs(300)).await;
        let conn = ConnectionServices::without_push(0);
        let mut write = Batch::new();
        write.insert("insert", insert("items").row(doc().set("marker", "vis1")));
        for query_version in [0, 1, 2, 99] {
            let request = DbRequest::Execute {
                query_version,
                db: "app".into(),
                batch: write.build(),
            };
            let bytes = rmp_serde::to_vec_named(&request).unwrap();
            let reply = handler.handle(&session, &bytes, &conn).await.unwrap();
            let response: DbResponse = rmp_serde::from_slice(&reply).unwrap();
            assert!(matches!(
                response,
                DbResponse::Error { code, .. } if code == "unsupported_query_version"
            ));
            for request in [
                DbRequest::TxBegin {
                    query_version,
                    db: "app".into(),
                    repo: "main".into(),
                    isolation: None,
                },
                DbRequest::TxExecute {
                    query_version,
                    db: "app".into(),
                    tx_handle: handle,
                    batch: write.build(),
                },
            ] {
                let bytes = rmp_serde::to_vec_named(&request).unwrap();
                let reply = handler.handle(&session, &bytes, &conn).await.unwrap();
                let response: DbResponse = rmp_serde::from_slice(&reply).unwrap();
                assert!(matches!(
                    response,
                    DbResponse::Error { code, .. } if code == "unsupported_query_version"
                ));
            }
        }
        match execute(&handler, &session, handle, read("items")).await {
            DbResponse::TxBatch { response } => {
                assert!(
                    response.results["read"].records.is_empty(),
                    "nothing staged"
                );
            }
            other => panic!("handle changed after version rejection: {other:?}"),
        }
        // Stage a real write: rejecting old COMMIT must leave it uncommitted
        // and preserve the handle, not merely reject an unknown transaction.
        assert!(matches!(
            execute(&handler, &session, handle, write.build()).await,
            DbResponse::TxBatch { .. }
        ));
        for query_version in [0, 1, 2, 99] {
            let request = DbRequest::TxCommit {
                query_version,
                visibility_wait: None,
                db: "app".into(),
                tx_handle: handle,
            };
            let bytes = rmp_serde::to_vec_named(&request).unwrap();
            let reply = handler.handle(&session, &bytes, &conn).await.unwrap();
            let response: DbResponse = rmp_serde::from_slice(&reply).unwrap();
            assert!(matches!(
                response,
                DbResponse::Error { code, .. } if code == "unsupported_query_version"
            ));
            assert!(handler
                .tx_registry
                .get_owned(handle, &session.session_id)
                .is_ok());
        }
        match handler
            .execute(
                &session,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                read("items"),
                &conn,
            )
            .await
        {
            DbResponse::Batch { response } => {
                assert!(
                    response.results["read"].records.is_empty(),
                    "no write committed"
                );
            }
            other => panic!("fresh read failed: {other:?}"),
        }
        handler.tx_rollback(&session, "app", handle).await;
    });
}

#[test]
fn vis1_tx_execute_rejects_commit_only_override_without_staging() {
    on_big_stack(|| async {
        let (handler, session, handle) = fixture(Duration::from_secs(300)).await;
        let mut write = Batch::new();
        write.insert(
            "insert",
            insert("items").row(doc().set("marker", "must-not-stage")),
        );
        for wait in [
            VisibilityWait::Strict,
            VisibilityWait::Bounded { timeout_ms: 0 },
        ] {
            write.visibility_wait(wait);
            match execute(&handler, &session, handle, write.build()).await {
                DbResponse::Error { code, .. } => assert_eq!(code, "validation"),
                other => panic!("commit-only override executed: {other:?}"),
            }
        }
        let mut inner = Batch::new();
        inner.visibility_wait(VisibilityWait::Strict);
        inner.insert("insert", insert("items").row(doc().set("marker", "nested")));
        let mut outer = Batch::new();
        outer.sub_batch_no_bind("nested", inner.build());
        match execute(&handler, &session, handle, outer.build()).await {
            DbResponse::Error { code, .. } => assert_eq!(code, "validation"),
            other => panic!("nested commit-only override executed: {other:?}"),
        }
        match execute(&handler, &session, handle, read("items")).await {
            DbResponse::TxBatch { response } => {
                assert!(
                    response.results["read"].records.is_empty(),
                    "nothing staged"
                );
                assert!(response.transaction.is_none());
            }
            other => panic!("handle was lost: {other:?}"),
        }
        handler.tx_rollback(&session, "app", handle).await;
    });
}
