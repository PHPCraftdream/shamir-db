use std::future::{poll_fn, Future};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::visibility::wait_for_visibility;
use shamir_query_builder::{batch::Batch, Query};
use shamir_query_types::batch::{VisibilityState, VisibilityWait};
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::config::TransactionsConfig;
use crate::db_handler::ShamirDbHandler;

fn session(id: [u8; 16]) -> Session {
    Session::new(
        id,
        "reader".into(),
        SessionPermissions::new(false, false, vec![]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        0,
    )
}

async fn send(handler: &ShamirDbHandler, session: &Session, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).unwrap();
    let conn = ConnectionServices::without_push(0);
    let bytes = handler.handle(session, &bytes, &conn).await.unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn visibility_wire_authorization_overrides_and_legacy_gate() {
    let owner = session([1; 16]);
    let stranger = session([2; 16]);
    let db = ShamirDb::init_memory().await.unwrap();
    let actor = Actor::User(principal64(owner.user_id));
    db.create_db_as("app", actor.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
        actor,
    )
    .await
    .unwrap();
    let handler = ShamirDbHandler::new(Arc::new(db));
    let status = VisibilityWait::Bounded { timeout_ms: 0 };
    let denied = send(
        &handler,
        &stranger,
        wait_for_visibility("app", "main", 0, status),
    )
    .await;
    assert!(matches!(denied, DbResponse::Error { code, .. } if code == "access_denied"));
    assert!(matches!(
        send(
            &handler,
            &owner,
            wait_for_visibility("app", "main", 0, status),
        )
        .await,
        DbResponse::VisibilityObserved {
            version: 0,
            visibility: VisibilityState::Visible,
        },
    ));
    let mut legacy = wait_for_visibility("missing", "missing", u64::MAX, status);
    if let DbRequest::WaitForVisibility { query_version, .. } = &mut legacy {
        *query_version = 2;
    }
    assert!(matches!(
        send(&handler, &owner, legacy).await,
        DbResponse::Error { code, .. } if code == "unsupported_query_version",
    ));
    assert!(matches!(
        send(
            &handler,
            &owner,
            wait_for_visibility("app", "main", 1, status),
        )
        .await,
        DbResponse::Error { code, .. } if code == "visibility_future_version",
    ));
    let handler = handler.with_transactions(TransactionsConfig {
        max_visibility_wait_ms: 0,
        allow_strict_visibility_wait: false,
        ..TransactionsConfig::default()
    });
    for wait in [
        VisibilityWait::Strict,
        VisibilityWait::Bounded { timeout_ms: 1 },
    ] {
        assert!(matches!(
            send(
                &handler,
                &owner,
                wait_for_visibility("app", "main", 0, wait),
            )
            .await,
            DbResponse::Error { code, .. } if code == "visibility_wait_invalid",
        ));
    }
    assert!(handler.tx_registry.is_empty());
}

#[tokio::test]
async fn configured_zero_as_of_budget_reaches_real_server_execution() {
    let owner = session([1; 16]);
    let db = ShamirDb::init_memory().await.unwrap();
    let actor = Actor::User(principal64(owner.user_id));
    db.create_db_as("app", actor.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        actor,
    )
    .await
    .unwrap();
    let gate = db
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_gate()
        .await
        .unwrap();
    db.get_db("app")
        .unwrap()
        .get_table("main", "items")
        .await
        .unwrap();
    let lower = gate.assign_next_version_guarded();
    let handler = ShamirDbHandler::new(Arc::new(db)).with_transactions(TransactionsConfig {
        read_visibility_timeout_ms: 0,
        ..TransactionsConfig::default()
    });
    let mut batch = Batch::new();
    batch.query("read", Query::with_repo("main", "items").as_of_version(1));
    // Execute's envelope is transport framing; the query itself is builder-produced.
    let response = send(
        &handler,
        &owner,
        DbRequest::Execute {
            query_version: 4,
            db: "app".into(),
            batch: batch.build(),
        },
    )
    .await;
    assert!(matches!(
        response,
        DbResponse::AsOfVisibilityTimeout {
            requested: 1,
            visible_through: 0,
            blocking_version: 1,
            code,
            ..
        } if code == "as_of_visibility_timeout",
    ));
    assert_eq!(gate.visible_floor(), 0);
    drop(lower);
}

#[tokio::test]
async fn interactive_as_of_timeout_preserves_handle_and_structured_fields() {
    let owner = session([1; 16]);
    let db = ShamirDb::init_memory().await.unwrap();
    let actor = Actor::User(principal64(owner.user_id));
    db.create_db_as("app", actor.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        actor,
    )
    .await
    .unwrap();
    let gate = db
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_gate()
        .await
        .unwrap();
    db.get_db("app")
        .unwrap()
        .get_table("main", "items")
        .await
        .unwrap();
    let lower = gate.assign_next_version_guarded();
    let handler = ShamirDbHandler::new(Arc::new(db)).with_transactions(TransactionsConfig {
        read_visibility_timeout_ms: 0,
        ..TransactionsConfig::default()
    });
    let opened = handler.tx_begin(&owner, 4, "app", "main", None).await;
    let handle = match opened {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("begin failed: {other:?}"),
    };
    let mut batch = Batch::new();
    batch.query("read", Query::with_repo("main", "items").as_of_version(1));
    let result = handler
        .tx_execute(&owner, 4, "app", handle, batch.build())
        .await;
    assert!(matches!(
        result,
        DbResponse::AsOfVisibilityTimeout {
            requested: 1,
            visible_through: 0,
            blocking_version: 1,
            ..
        },
    ));
    assert!(handler
        .tx_registry
        .get_owned(handle, &owner.session_id)
        .is_ok());
    drop(lower);
    assert!(matches!(
        handler.tx_rollback(&owner, "app", handle).await,
        DbResponse::TxRolledBack { tx_handle } if tx_handle == handle,
    ));
    assert!(handler.tx_registry.is_empty());
}

#[tokio::test(start_paused = true)]
async fn configured_nonzero_budget_times_out_at_server_read_seam() {
    let owner = session([1; 16]);
    let db = ShamirDb::init_memory().await.unwrap();
    let actor = Actor::User(principal64(owner.user_id));
    db.create_db_as("app", actor.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        actor,
    )
    .await
    .unwrap();
    let gate = db
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_gate()
        .await
        .unwrap();
    db.get_db("app")
        .unwrap()
        .get_table("main", "items")
        .await
        .unwrap();
    let lower = gate.assign_next_version_guarded();
    let handler = ShamirDbHandler::new(Arc::new(db)).with_transactions(TransactionsConfig {
        read_visibility_timeout_ms: 17,
        ..TransactionsConfig::default()
    });
    let mut batch = Batch::new();
    batch.query("read", Query::with_repo("main", "items").as_of_version(1));
    let conn = ConnectionServices::without_push(0);
    let mut execution = Box::pin(handler.execute(&owner, 4, "app", batch.build(), &conn));
    poll_fn(|cx| {
        assert!(execution.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_millis(16)).await;
    poll_fn(|cx| {
        assert!(execution.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(
        execution.await,
        DbResponse::AsOfVisibilityTimeout {
            requested: 1,
            visible_through: 0,
            blocking_version: 1,
            ..
        },
    ));
    assert_eq!(gate.visible_floor(), 0);
    drop(lower);
}
