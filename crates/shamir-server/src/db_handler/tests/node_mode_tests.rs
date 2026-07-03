//! Tests for the read-only replica gate in `ShamirDbHandler::execute`
//! (REPLICATION PR4 §4.3).
//!
//! The handler carries a [`NodeMode`]. When `ReadOnly`, any batch entry
//! whose [`BatchOp::is_write`] returns `true` is rejected with
//! `code = "read_only_replica"` *before* the batch reaches the engine.
//! Reads (SELECT, introspection) pass through. The default (`ReadWrite`)
//! leaves behaviour unchanged.
//!
//! Queries are built exclusively through `shamir-query-builder` (CLAUDE.md).

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal_id, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::{insert, upsert};
use shamir_types::mpack;

use crate::db_handler::config::NodeMode;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A regular ("alice") session — resolves to `Actor::User(principal_id("alice"))`.
/// Mirrors the bench fixture in `benches/wire_pipelining.rs`.
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

/// Build a handler over an in-memory `ShamirDb` with a `main` repo containing
/// an `items` table, owned by `alice` (so the non-superuser session can pass
/// the Shomer gate on subsequent ops). See `wire_pipelining.rs::build_handler`.
async fn build_handler(mode: NodeMode) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    // `create_db`/`add_repo` (System-owned) persist ResourceMeta::owned_enforced
    // (owner-only 0o700); alice owns the resources so her session passes the gate.
    let owner = Actor::User(principal_id("alice"));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    ShamirDbHandler::new(Arc::new(shamir)).with_node_mode(mode)
}

// ---------------------------------------------------------------------------
// Batch builders
// ---------------------------------------------------------------------------

/// A write batch: a single upsert of key `k1` into `items`.
fn write_batch_upsert() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.upsert(
        "w1",
        upsert("items")
            .key(mpack!({ "id": "k1" }))
            .value(doc! { "id" => "k1", "v" => 1_i64 }),
    );
    b.build()
}

/// A write batch: a single insert into `items`.
fn write_batch_insert() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.insert(
        "i1",
        insert("items").row(doc! { "id" => "k2", "v" => 2_i64 }),
    );
    b.build()
}

/// A read-only batch: a single SELECT from `items`.
fn read_batch_select() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.query("r1", Query::from("items"));
    b.build()
}

/// A mixed batch: a read (SELECT) followed by an insert.
fn mixed_batch() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.query("r1", Query::from("items"));
    b.insert(
        "i1",
        insert("items").row(doc! { "id" => "k3", "v" => 3_i64 }),
    );
    b.build()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Case 1 — ReadWrite (default) + write batch (upsert) → success.
/// Zero behavioural change: the gate is a no-op for `ReadWrite`.
#[tokio::test]
async fn readwrite_allows_write_batch() {
    let handler = build_handler(NodeMode::ReadWrite).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            write_batch_upsert(),
            &conn,
        )
        .await;

    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "ReadWrite node should accept writes, got: {resp:?}",
    );
}

/// Case 2 — ReadOnly + write batch (insert) → `read_only_replica` error.
#[tokio::test]
async fn readonly_rejects_write_batch() {
    let handler = build_handler(NodeMode::ReadOnly).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            write_batch_insert(),
            &conn,
        )
        .await;

    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "read_only_replica", "wrong code; message: {message}");
            assert!(
                message.contains("is a write") && message.contains("read-only replica"),
                "unexpected message: {message}",
            );
        }
        other => panic!("ReadOnly node should reject writes, got: {other:?}"),
    }
}

/// Case 3 — ReadOnly + read-only batch (SELECT) → success (reads pass).
#[tokio::test]
async fn readonly_allows_read_batch() {
    let handler = build_handler(NodeMode::ReadOnly).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            read_batch_select(),
            &conn,
        )
        .await;

    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "ReadOnly node should accept reads, got: {resp:?}",
    );
}

/// Case 4 — ReadOnly + mixed batch (read + insert) → rejected; any write
/// in the batch fails the whole batch at the gate.
#[tokio::test]
async fn readonly_rejects_mixed_batch() {
    let handler = build_handler(NodeMode::ReadOnly).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            mixed_batch(),
            &conn,
        )
        .await;

    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "read_only_replica", "wrong code; message: {message}");
        }
        other => panic!("ReadOnly node should reject mixed batch, got: {other:?}"),
    }
}
