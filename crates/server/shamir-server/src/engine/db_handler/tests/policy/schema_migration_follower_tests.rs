//! Follower refusal for application-schema operations (R1c 2b-1, design
//! §4.5): a `ReadOnly` (follower/read-replica) node refuses EVERY
//! `BatchOp::ApplicationSchema` arm — including the read-only status arm —
//! with the stable code `schema_migration_follower_read_only`, before the
//! generic read-only-replica classification. On a `ReadWrite` node the
//! status op passes the gate and executes against the engine.
//!
//! Requests are built exclusively through `shamir-query-builder` (CLAUDE.md).

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::ShamirDb;

use shamir_query_builder::application_schema_plan;
use shamir_query_builder::application_schema_status;
use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationPlanRequest, SchemaMigrationPredecessor, SchemaMigrationStatusRequest,
    SchemaMigrationTarget,
};

use crate::db_handler::config::NodeMode;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

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

/// Handler over an in-memory `ShamirDb` with a database "app" owned by the
/// alice projection, so the status Read check admits her on `ReadWrite`.
async fn build_handler(mode: NodeMode) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner).await.expect("create_db");
    ShamirDbHandler::new(Arc::new(shamir)).with_node_mode(mode)
}

fn status_batch() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.op(
        "status",
        application_schema_status(SchemaMigrationStatusRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: shamir_types::types::record_id::RecordId::from_ts_seq(
                    1_700_000_000_000_000,
                    7,
                ),
            },
        }),
    );
    b.build()
}

#[tokio::test]
async fn follower_refuses_application_schema_status_as_stale() {
    let handler = build_handler(NodeMode::ReadOnly).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            status_batch(),
            &conn,
        )
        .await;

    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "schema_migration_follower_read_only",
                "wrong code; message: {message}"
            );
        }
        other => panic!("follower must refuse status, got: {other:?}"),
    }
}

/// Plan-for-apply is primary-only (design §4.5): the same single
/// `BatchOp::ApplicationSchema` gate refuses it on a ReadOnly node.
fn plan_batch() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.op(
        "plan",
        application_schema_plan(SchemaMigrationPlanRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: shamir_types::types::record_id::RecordId::from_ts_seq(
                    1_700_000_000_000_000,
                    7,
                ),
            },
            expected_predecessor: SchemaMigrationPredecessor {
                migration_id: None,
                ordinal: 0,
                checksum: [0; 32],
            },
            migrations: Vec::new(),
        }),
    );
    b.build()
}

#[tokio::test]
async fn follower_refuses_application_schema_plan_as_stale() {
    let handler = build_handler(NodeMode::ReadOnly).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            plan_batch(),
            &conn,
        )
        .await;

    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "schema_migration_follower_read_only",
                "wrong code; message: {message}"
            );
        }
        other => panic!("follower must refuse plan, got: {other:?}"),
    }
}

#[tokio::test]
async fn read_write_node_serves_application_schema_status() {
    let handler = build_handler(NodeMode::ReadWrite).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            status_batch(),
            &conn,
        )
        .await;

    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "ReadWrite node should serve status, got: {resp:?}"
    );
}
