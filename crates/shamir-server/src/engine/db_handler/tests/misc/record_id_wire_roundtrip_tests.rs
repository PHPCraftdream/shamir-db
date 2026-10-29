//! RU6 handler-level round trip (RV-QUERY-TYPES-A984DFED, acceptance item
//! 6): the response's `op_id` decodes as a plain STRING through a
//! non-Rust-typed mirror (how the TS client sees it) and that string, fed
//! unchanged into `DbRequest::GetDdlOpStatus` through the real handler,
//! resolves `Some(status)` with the same id and a terminal state.
//!
//! The DROP is executed through the HMAC-gated handler and response envelope.

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::SessionPermissions;

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex};
use shamir_query_types::read::DdlOpState;
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

/// Non-Rust-typed mirror of `QueryResult` — only the poll token matters.
#[derive(serde::Deserialize)]
struct OpIdMirror {
    op_id: String,
}

async fn mint_dropped_index_op_id() -> (ShamirDbHandler, String) {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    let table = shamir
        .get_db("app")
        .expect("db exists")
        .get_table("main", "items")
        .await
        .expect("table exists");
    table
        .create_index("idx_city", &["city"])
        .await
        .expect("create index");

    let request_id =
        shamir_types::types::record_id::RecordId::try_from_bytes(&(1u8..=16).collect::<Vec<_>>())
            .expect("request id");
    let mut op = ddl::drop_index("idx_city", "items")
        .repo("main")
        .request_id(request_id)
        .hmac("placeholder")
        .build()
        .expect("build drop index");
    let tag = compute_tag_hex(
        &owner_session().hmac_key(),
        &canonical_batch_op("app", &op).expect("canonical drop"),
    );
    if let BatchOp::DropIndex(ref mut drop) = op {
        drop.hmac = Some(tag);
    } else {
        panic!("expected drop index");
    }
    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", op);
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let resp = send(
        &handler,
        DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: b.build(),
        },
    )
    .await;
    let DbResponse::Batch { response } = resp else {
        panic!("expected batch response: {resp:?}");
    };
    let bytes = rmp_serde::to_vec_named(&response.results["d"]).expect("encode response");
    let mirror: OpIdMirror = rmp_serde::from_slice(&bytes).expect("mirror decode");
    assert_eq!(mirror.op_id, request_id.to_string());
    assert!(
        mirror
            .op_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() && !"0OIl".contains(c)),
        "op_id must be a base58 string, got {}",
        mirror.op_id
    );

    (handler, mirror.op_id)
}

fn owner_session() -> shamir_connect::server::session::Session {
    let mut s = shamir_connect::server::session::Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(true, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = [0xAA; 32];
    s
}

async fn send(handler: &ShamirDbHandler, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let resp_bytes = handler
        .handle(&owner_session(), &bytes, &conn)
        .await
        .expect("handle must not error at the protocol level");
    rmp_serde::from_slice(&resp_bytes).expect("decode response")
}

#[tokio::test]
async fn rv_query_types_a984dfed_poll_by_response_string_through_handler() {
    let (handler, op_id) = mint_dropped_index_op_id().await;

    let resp = send(
        &handler,
        DbRequest::GetDdlOpStatus {
            db: "app".to_string(),
            repo: "main".to_string(),
            table: "items".to_string(),
            op_id: op_id.clone(),
        },
    )
    .await;

    match resp {
        DbResponse::DdlOpStatus {
            status: Some(status),
        } => {
            assert_eq!(status.op_id.to_string(), op_id);
            assert!(
                matches!(status.state, DdlOpState::Succeeded { .. }),
                "expected terminal Succeeded state, got {:?}",
                status.state
            );
        }
        other => panic!("expected DdlOpStatus, got {other:?}"),
    }
}
