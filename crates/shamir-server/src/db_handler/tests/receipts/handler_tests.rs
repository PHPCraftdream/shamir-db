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
use shamir_query_builder::write::{doc, Insert};
use shamir_query_types::batch::{VisibilityState, VisibilityWait};
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::byte_budget::{run_with_guard_slot, take_stashed_guard, ByteBudget};
use crate::config::TransactionsConfig;
use crate::db_handler::ShamirDbHandler;

// Red before VIS-4b: startup default rejected / implicit outcomes discarded.
#[tokio::test(start_paused = true)]
async fn bounded_startup_default_applies_and_pending_reservation_shrinks_then_releases() {
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
    db.create_db_as("app", actor.clone()).await;
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        actor,
    )
    .await
    .unwrap();
    db.get_db("app")
        .unwrap()
        .get_table("main", "items")
        .await
        .unwrap();
    let repo = db.get_db("app").unwrap().get_repo("main").unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let lower = gate.assign_next_version_guarded();
    let config = TransactionsConfig {
        visibility_ack_mode: "bounded".into(),
        visibility_ack_timeout_ms: 0,
        ..TransactionsConfig::default()
    };
    config.validate().unwrap();
    assert_eq!(
        config.resolve_commit_wait(None).unwrap(),
        VisibilityWait::Bounded { timeout_ms: 0 },
    );
    let budget = ByteBudget::new(Some(16 * 1024 * 1024));
    let handler = ShamirDbHandler::new(Arc::new(db))
        .with_transactions(config)
        .with_byte_budget(budget.clone());
    let mut batch = Batch::new();
    batch.insert(
        "written",
        Insert::with_repo("main", "items").row(doc().set("n", 1)),
    );
    let request = DbRequest::Execute {
        query_version: 4,
        db: "app".into(),
        batch: batch.build(),
    };
    let bytes = rmp_serde::to_vec_named(&request).unwrap();
    let (wire, guard) = tokio::spawn(async move {
        run_with_guard_slot(async {
            let conn = ConnectionServices::without_push(0);
            let wire = handler.handle(&session, &bytes, &conn).await.unwrap();
            (wire, take_stashed_guard())
        })
        .await
    })
    .await
    .unwrap();
    let response: DbResponse = rmp_serde::from_slice(&wire).unwrap();
    let DbResponse::Batch { response } = response else {
        panic!("expected batch");
    };
    let receipt = response.results["written"].receipt.as_ref().unwrap();
    assert!(receipt.is_committed());
    assert!(matches!(
        receipt.visibility,
        Some(VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            ..
        })
    ));
    assert_eq!(budget.used(), wire.len());
    assert!(budget.used() < 16 * 1024 * 1024);
    drop(guard.expect("pending response follows writer guard ownership"));
    assert_eq!(budget.used(), 0);
    assert!(gate.active_snapshots_empty());
    lower.commit();
}
