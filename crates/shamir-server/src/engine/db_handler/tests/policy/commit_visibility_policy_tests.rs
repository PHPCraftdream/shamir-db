//! VIS-3 real-handler regressions; queries are builder-produced, envelopes are wire framing.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::val::lit;
use shamir_query_builder::visibility::wait_for_visibility;
use shamir_query_builder::write::{doc, Insert};
use shamir_query_types::batch::{
    BatchRequest, VisibilityPendingReason, VisibilityState, VisibilityWait,
};
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::config::TransactionsConfig;
use crate::db_handler::ShamirDbHandler;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

async fn fixture() -> (Arc<ShamirDbHandler>, Arc<Session>) {
    let session = Arc::new(Session::new(
        [83; 16],
        "owner".into(),
        SessionPermissions::new(false, false, vec![]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        0,
    ));
    let db = ShamirDb::init_memory().await.unwrap();
    let actor = Actor::User(principal64(session.user_id));
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
    // Initialize the table before allocating the artificial lower blocker.
    db.get_db("app")
        .unwrap()
        .get_table("main", "items")
        .await
        .unwrap();
    let handler = ShamirDbHandler::new(Arc::new(db)).with_transactions(TransactionsConfig {
        max_visibility_wait_ms: 0,
        visibility_ack_timeout_ms: 0,
        allow_strict_visibility_wait: false,
        ..TransactionsConfig::default()
    });
    (Arc::new(handler), session)
}

async fn send(handler: &ShamirDbHandler, session: &Session, request: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&request).unwrap();
    let conn = ConnectionServices::without_push(0);
    let response = handler.handle(session, &bytes, &conn).await.unwrap();
    rmp_serde::from_slice(&response).unwrap()
}

fn writes(wait: Option<VisibilityWait>, transactional: bool) -> BatchRequest {
    let mut batch = Batch::new();
    if transactional {
        batch.transactional();
    }
    if let Some(wait) = wait {
        batch.visibility_wait(wait);
    }
    batch.insert(
        "written",
        Insert::with_repo("main", "items").row(doc().set("name", "receipt")),
    );
    batch.build()
}

fn execute(batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: 4,
        db: "app".into(),
        batch,
    }
}

fn commit(handle: u64, wait: Option<VisibilityWait>) -> DbRequest {
    DbRequest::TxCommit {
        query_version: 4,
        db: "app".into(),
        tx_handle: handle,
        visibility_wait: wait,
    }
}

async fn begin(handler: &ShamirDbHandler, session: &Session) -> u64 {
    match handler.tx_begin(session, 4, "app", "main", None).await {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("begin failed: {other:?}"),
    }
}

fn invalid(response: DbResponse) {
    assert!(
        matches!(&response, DbResponse::Error { code, .. } if code == "visibility_wait_invalid"),
        "{response:?}"
    );
}

// Red before VIS-3: TxCommit ignored the budget and parked; no pending receipt returned.
#[tokio::test(start_paused = true)]
async fn pending_commit_consumes_handle_releases_snapshot_and_reconciles_over_wire() {
    let (handler, session) = fixture().await;
    let repo = handler
        .db()
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let lower = gate.assign_next_version_guarded();
    let handle = begin(&handler, &session).await;
    assert!(!gate.active_snapshots_empty());
    let staged = handler
        .tx_execute(&session, 4, "app", handle, writes(None, false))
        .await;
    assert!(matches!(&staged, DbResponse::TxBatch { .. }), "{staged:?}");
    let owned = handler.clone();
    let caller = session.clone();
    let task =
        tokio::spawn(async move { send(&owned, &caller, commit(handle, Some(STATUS))).await });
    let info = match task.await.unwrap() {
        DbResponse::TxCommitted { transaction } => transaction,
        other => panic!("commit failed: {other:?}"),
    };
    assert!(info.is_committed());
    assert!(!info.is_visible());
    assert!(info.materialized);
    assert_eq!(
        info.visibility,
        Some(VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::Timeout,
        })
    );
    assert!(handler.tx_registry().is_empty());
    assert!(gate.active_snapshots_empty());
    assert_eq!(gate.visible_floor(), 0);
    assert!(
        matches!(send(&handler, &session, commit(handle, Some(STATUS))).await,
        DbResponse::Error { code, .. } if code == "tx_not_found")
    );
    lower.commit();
    assert!(matches!(
        send(
            &handler,
            &session,
            wait_for_visibility("app", "main", info.commit_version.unwrap(), STATUS)
        )
        .await,
        DbResponse::VisibilityObserved {
            visibility: VisibilityState::Visible,
            ..
        }
    ));
}

// Red before server forwarding: the transactional request did not resolve its policy.
#[tokio::test(start_paused = true)]
async fn zero_bounded_transactional_execute_returns_pending_receipt_over_wire() {
    let (handler, session) = fixture().await;
    let repo = handler
        .db()
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let lower = gate.assign_next_version_guarded();
    let owned = handler.clone();
    let caller = session.clone();
    let task =
        tokio::spawn(
            async move { send(&owned, &caller, execute(writes(Some(STATUS), true))).await },
        );
    let info = match task.await.unwrap() {
        DbResponse::Batch { response } => {
            assert!(response.results.contains_key("written"));
            response.transaction.unwrap()
        }
        other => panic!("transactional Execute failed: {other:?}"),
    };
    assert!(info.is_committed());
    assert_eq!(
        info.visibility,
        Some(VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::Timeout,
        })
    );
    assert_eq!(info.commit_version, Some(2));
    assert!(gate.active_snapshots_empty());
    lower.commit();
    assert_eq!(gate.visible_floor(), 2);
}

#[test]
fn bounded_server_default_is_enabled() {
    let config = TransactionsConfig {
        visibility_ack_mode: "bounded".into(),
        ..TransactionsConfig::default()
    };
    config.validate().unwrap();
    assert_eq!(
        config.resolve_commit_wait(None).unwrap(),
        VisibilityWait::Bounded { timeout_ms: 5000 },
    );
}

// Red before the server policy fix: excessive/forbidden waits were ignored and wrote.
#[tokio::test]
async fn execute_validates_all_overrides_before_writes_and_default_strict_is_not_forbidden() {
    let (handler, session) = fixture().await;
    let repo = handler
        .db()
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap();
    let gate = repo.tx_gate().await.unwrap();
    for wait in [
        VisibilityWait::Strict,
        VisibilityWait::Bounded { timeout_ms: 1 },
    ] {
        invalid(send(&handler, &session, execute(writes(Some(wait), true))).await);
        assert_eq!(gate.version_allocation_high_water_mark(), 0);
    }
    // Permission gates explicit Strict only; omission is the configured default.
    let response = send(&handler, &session, execute(writes(None, true))).await;
    match response {
        DbResponse::Batch { response } => {
            assert!(response.results.contains_key("written"));
            assert!(response.transaction.unwrap().is_visible());
        }
        other => panic!("default Strict rejected: {other:?}"),
    }
    assert_eq!(gate.version_allocation_high_water_mark(), 1);
    let response = send(&handler, &session, execute(writes(Some(STATUS), true))).await;
    match response {
        DbResponse::Batch { response } => assert!(response.transaction.unwrap().is_visible()),
        other => panic!("zero bounded transactional request rejected: {other:?}"),
    }
    assert_eq!(gate.version_allocation_high_water_mark(), 2);
}

#[tokio::test]
async fn recursive_body_invalid_overrides_fail_closed_before_writes() {
    let (handler, session) = fixture().await;
    let repo = handler
        .db()
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap();
    let gate = repo.tx_gate().await.unwrap();
    for wait in [
        VisibilityWait::Strict,
        VisibilityWait::Bounded { timeout_ms: 1 },
    ] {
        for body_transactional in [false, true] {
            let mut middle = Batch::new();
            middle.for_each(
                "loop",
                vec![lit(1)],
                "row",
                writes(Some(wait), body_transactional),
            );
            let mut outer = Batch::new();
            // A write preceding the invalid subtree must never execute.
            outer.insert(
                "first",
                Insert::with_repo("main", "items").row(doc().set("name", "first")),
            );
            outer.sub_batch_no_bind("nested", middle.build());
            invalid(send(&handler, &session, execute(outer.build())).await);
            assert_eq!(gate.version_allocation_high_water_mark(), 0);
        }
    }
    assert!(gate.active_snapshots_empty());
}

#[tokio::test]
async fn invalid_commit_policy_consumes_owned_handle_but_version_and_owner_gates_do_not() {
    let (handler, session) = fixture().await;
    let repo = handler
        .db()
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap();
    let gate = repo.tx_gate().await.unwrap();
    for wait in [
        VisibilityWait::Strict,
        VisibilityWait::Bounded { timeout_ms: 1 },
    ] {
        let handle = begin(&handler, &session).await;
        let staged = handler
            .tx_execute(&session, 4, "app", handle, writes(None, false))
            .await;
        assert!(matches!(staged, DbResponse::TxBatch { .. }));
        let mut legacy = commit(handle, Some(wait));
        if let DbRequest::TxCommit { query_version, .. } = &mut legacy {
            *query_version = 2;
        }
        assert!(matches!(send(&handler, &session, legacy).await,
            DbResponse::Error { code, .. } if code == "unsupported_query_version"));
        assert!(!gate.active_snapshots_empty());
        // Session::new leaves session_id zeroed; ownership is keyed by it.
        let mut stranger = Session::new(
            [84; 16],
            "stranger".into(),
            SessionPermissions::new(false, false, vec![]),
            TransportKind::Tcp,
            BindingMode::TlsExporter,
            [0; 32],
            0,
        );
        stranger.session_id = [84; 32];
        assert!(
            matches!(send(&handler, &stranger, commit(handle, Some(wait))).await,
            DbResponse::Error { code, .. } if code == "tx_forbidden")
        );
        assert!(!handler.tx_registry().is_empty());
        invalid(send(&handler, &session, commit(handle, Some(wait))).await);
        assert!(handler.tx_registry().is_empty());
        assert!(gate.active_snapshots_empty());
        assert_eq!(gate.version_allocation_high_water_mark(), 0);
    }
    let handle = begin(&handler, &session).await;
    assert!(
        matches!(send(&handler, &session, commit(handle, None)).await,
        DbResponse::TxCommitted { transaction } if transaction.is_visible())
    );
    assert!(gate.active_snapshots_empty());
}

#[tokio::test]
async fn tx_execute_still_rejects_commit_only_options_without_consuming_handle() {
    let (handler, session) = fixture().await;
    let repo = handler
        .db()
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let handle = begin(&handler, &session).await;
    for nested in [false, true] {
        let batch = if nested {
            let mut outer = Batch::new();
            outer.sub_batch_no_bind("nested", writes(Some(STATUS), false));
            outer.build()
        } else {
            writes(Some(STATUS), false)
        };
        let response = send(
            &handler,
            &session,
            DbRequest::TxExecute {
                query_version: 4,
                db: "app".into(),
                tx_handle: handle,
                batch,
            },
        )
        .await;
        assert!(matches!(response, DbResponse::Error { code, .. } if code == "validation"));
        assert!(!handler.tx_registry().is_empty());
        assert_eq!(gate.version_allocation_high_water_mark(), 0);
    }
    handler.tx_rollback(&session, "app", handle).await;
    assert!(gate.active_snapshots_empty());
}
