//! Encoded error strings are unbounded too; refusal must not retain them.

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::Query;
use shamir_query_types::wire::DbResponse;

use crate::byte_budget::{
    run_with_guard_slot, take_stashed_guard, take_stashed_serialized_response, ByteBudget,
};
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

// Red before W10: bounded mode grows and stashes this oversized error;
// unbounded mode never checks its encoded size at all.
#[tokio::test]
async fn encoded_oversize_refuses_and_releases_with_or_without_global_budget() {
    for cap in [Some(1024), None] {
        let db = ShamirDb::init_memory().await.unwrap();
        let actor = Actor::User(principal64([0xAB; 16]));
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
        let budget = ByteBudget::new(cap);
        let handler = ShamirDbHandler::new(Arc::new(db))
            .with_byte_budget(budget.clone())
            .with_query_limits(QueryLimitsCap {
                max_result_size_bytes: 1024,
                ..QueryLimitsCap::UNLIMITED
            });
        let session = Session::new(
            [0xAB; 16],
            "alice".into(),
            SessionPermissions::from_roles(vec!["read_write".into()]),
            TransportKind::Tcp,
            BindingMode::TlsExporter,
            [0u8; 32],
            UnixNanos::now().as_u64(),
        );
        let conn = ConnectionServices::without_push(0);
        let mut batch = Batch::new();
        batch.query("r", Query::from("x".repeat(10_240)));
        run_with_guard_slot(async {
            let response = handler
                .execute(
                    &session,
                    CURRENT_QUERY_LANG_VERSION,
                    "app",
                    batch.build(),
                    &conn,
                )
                .await;
            assert!(
                matches!(&response, DbResponse::Error { code, .. } if code == "result_too_large"),
                "{response:?}",
            );
            assert_eq!(budget.used(), 0);
            assert!(take_stashed_guard().is_none());
            assert!(take_stashed_serialized_response().is_none());
        })
        .await;
    }
}
