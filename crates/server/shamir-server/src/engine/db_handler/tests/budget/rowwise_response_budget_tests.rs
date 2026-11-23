//! Encoded error strings are unbounded too; refusal must not retain them.

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest};
use shamir_query_types::wire::{DbRequest, DbResponse};

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

// ---------------------------------------------------------------------------
// The CALLER's own `max_result_size` may shrink a DATA reply, never a
// refusal: a coded error envelope keeps its own code.
// ---------------------------------------------------------------------------

fn alice_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// `app.main.items` holding ONE row whose 256-byte `pad` value cannot fit
/// any tiny result cap, served by a handler whose OPERATOR cap
/// (`query_limits.max_result_size_bytes`) is `result_cap` bytes; every other
/// operator cap stays `UNLIMITED`. The seed insert runs on its own unlimited
/// handler — the reply under test is the read that follows.
async fn seeded_handler(result_cap: usize) -> ShamirDbHandler {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        owner,
    )
    .await
    .expect("repo");
    let db = Arc::new(db);

    let mut seed = Batch::new();
    seed.insert(
        "seed",
        insert("items").row(doc! { "id" => "k0", "pad" => "x".repeat(256) }),
    );
    let seeder = ShamirDbHandler::new(Arc::clone(&db))
        .with_query_limits(QueryLimitsCap::UNLIMITED)
        .with_byte_budget(ByteBudget::unbounded());
    let seeded = seeder
        .execute(
            &alice_session(),
            CURRENT_QUERY_LANG_VERSION,
            "app",
            seed.build(),
            &ConnectionServices::without_push(0),
        )
        .await;
    assert!(
        matches!(seeded, DbResponse::Batch { .. }),
        "seed insert must succeed: {seeded:?}",
    );

    ShamirDbHandler::new(db)
        .with_query_limits(QueryLimitsCap {
            max_result_size_bytes: result_cap,
            ..QueryLimitsCap::UNLIMITED
        })
        .with_byte_budget(ByteBudget::unbounded())
}

/// Dispatch one `Execute` request over the real wire entry point
/// (`RequestHandler::handle`) and decode the reply it would write.
async fn dispatch(handler: &ShamirDbHandler, batch: BatchRequest) -> DbResponse {
    let req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        batch,
    };
    let req_bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);
    run_with_guard_slot(async {
        let reply = handler
            .handle(&session, &req_bytes, &conn)
            .await
            .expect("handle must not error at the protocol level");
        let _ = take_stashed_guard();
        rmp_serde::from_slice(&reply).expect("decode reply")
    })
    .await
}

/// The caller's own result cap must not destroy the cause of a refusal. A
/// read of an absent table fails with the coded engine error `unknown_table`;
/// with `limits.max_result_size = 1` the encoded-reply cap used to rewrite
/// that envelope into `result_too_large`.
#[tokio::test]
async fn tiny_caller_result_cap_keeps_the_fixed_error_envelope_and_its_code() {
    // The OPERATOR cap comfortably fits the envelope — only the caller's own
    // 1-byte limit makes it look "too large".
    let handler = seeded_handler(4096).await;
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.query("missing", Query::from("absent_table"));

    match dispatch(&handler, batch.build()).await {
        DbResponse::Error { code, message } => assert_eq!(
            code, "unknown_table",
            "a fixed refusal must keep its own code instead of result_too_large: {message}",
        ),
        other => panic!("expected the coded refusal, got {other:?}"),
    }
}

/// Negative control for the exemption above: a DATA reply that really is
/// larger than the caller's cap is STILL refused with `result_too_large`.
/// Without this, "exempt errors" could silently drift into "cap nothing".
#[tokio::test]
async fn tiny_caller_result_cap_still_refuses_a_larger_data_reply() {
    let handler = seeded_handler(4096).await;
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.query("row", Query::from("items"));

    match dispatch(&handler, batch.build()).await {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "result_too_large", "{message}");
            assert!(
                message.contains("maximum is 1"),
                "the refusal must name the CALLER's cap, not the operator's: {message}",
            );
        }
        other => panic!("expected the encoded DATA reply to be refused, got {other:?}"),
    }
}
