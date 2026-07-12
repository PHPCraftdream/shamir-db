//! HMAC-gate integration tests.
//!
//! Every destructive admin op (drop_db, drop_repo, drop_table,
//! drop_index, drop_user, drop_role) MUST carry a hex-encoded
//! HMAC-SHA256 tag over the canonical bytes for that op, keyed by
//! `SHA256("shamir-db hmac key v1\0" || session_id)`.
//!
//! These tests confirm the gate at the wire boundary:
//!   * missing tag → `hmac_required`
//!   * wrong tag   → `hmac_mismatch`
//!   * correct tag → op executes normally
//!
//! Non-destructive ops are untouched by the gate.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::Query;
use shamir_query_types::admin::PurgeScope;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

// --------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------

/// Build a superuser session. Bypassing the SessionStore means
/// `session_id` stays at zeros, which is fine — what matters for
/// HMAC validation is that the test computes its tag with the
/// SAME `session_id` the server sees.
fn root_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["superuser".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

fn session_key(session: &Session) -> [u8; 32] {
    canon::derive_session_hmac_key(&session.session_id)
}

async fn make_db_with_table(db: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    Arc::new(shamir)
}

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

fn execute_built(db: &str, batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

fn expect_error(res: DbResponse) -> (String, String) {
    match res {
        DbResponse::Error { code, message } => (code, message),
        other => panic!("expected Error, got {:?}", other),
    }
}

fn expect_batch_ok(res: DbResponse) -> shamir_db::query::batch::BatchResponse {
    match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch, got {:?}", other),
    }
}

// --------------------------------------------------------------------------
// drop_table
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_without_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_table("d", ddl::drop_table("items").repo("main"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _msg) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_with_wrong_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        ddl::drop_table("items")
            .repo("main")
            .hmac("deadbeef".repeat(8)), // 64 hex chars but bogus
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _msg) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_with_correct_hmac_accepted() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_drop_table("prod", "main", "items"));

    let mut b = Batch::new();
    b.id(1);
    b.drop_table("d", ddl::drop_table("items").repo("main").hmac(&tag));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["d"].records[0];
    assert_eq!(rec.get_value_str("dropped_table"), Some("items"));
    assert_eq!(rec.get_value_bool("existed"), Some(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_tag_bound_to_target_table() {
    // A tag computed for table A must not work against table B.
    let shamir = make_db_with_table("prod", "items").await;
    // Also seed a second table inside the same repo.
    {
        let db = shamir.get_db("prod").unwrap();
        db.create_table("main", "other").unwrap();
    }
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let key = session_key(&session);
    let tag_for_items =
        canon::compute_tag_hex(&key, &canon::canonical_drop_table("prod", "main", "items"));

    // Submit drop_table for "other" with the items-tag.
    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        ddl::drop_table("other").repo("main").hmac(&tag_for_items),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

// --------------------------------------------------------------------------
// drop_db
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_db_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    shamir.create_db("victim").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_db("d", ddl::drop_db("victim"));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_db_with_correct_hmac_accepted() {
    // Need a routing db that exists — ShamirDb::execute(db_name, ..)
    // looks up `db_name` BEFORE dispatching the batch. The drop_db op
    // can target any db; we route the batch through `scratch` so the
    // lookup succeeds, and drop `victim` inside the op.
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    shamir.create_db("victim").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let tag = canon::compute_tag_hex(&session_key(&session), &canon::canonical_drop_db("victim"));
    let mut b = Batch::new();
    b.id(1);
    b.drop_db("d", ddl::drop_db("victim").hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["d"].records[0];
    assert_eq!(rec.get_value_str("dropped"), Some("victim"));
}

// --------------------------------------------------------------------------
// drop_index
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_index_without_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // Pre-create an index (no HMAC needed for create_index).
    let mut b = Batch::new();
    b.id(0);
    b.create_index("i", ddl::create_index("by_id", "items").field("id"));
    let mk = execute_built("prod", b.build());
    let _ = handler
        .handle(&session, &encode(&mk), &ConnectionServices::without_push(0))
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("by_id", "items"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_index_with_correct_hmac_accepted() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(0);
    b.create_index("i", ddl::create_index("by_id", "items").field("id"));
    let mk = execute_built("prod", b.build());
    let _ = handler
        .handle(&session, &encode(&mk), &ConnectionServices::without_push(0))
        .await
        .unwrap();

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_drop_index("prod", "main", "items", "by_id", false),
    );
    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("by_id", "items").hmac(&tag));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["d"].records[0];
    assert_eq!(rec.get_value_str("dropped_index"), Some("by_id"));
    assert_eq!(rec.get_value_bool("existed"), Some(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_index_unique_flag_changes_canonical() {
    // unique=true vs unique=false must produce different tags. If
    // a client signs the non-unique form but sends unique=true, the
    // gate must refuse.
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(0);
    b.create_index(
        "i",
        ddl::create_index("by_em", "items").field("email").unique(),
    );
    let mk = execute_built("prod", b.build());
    let _ = handler
        .handle(&session, &encode(&mk), &ConnectionServices::without_push(0))
        .await
        .unwrap();

    // Tag computed for unique=false but request says unique=true.
    let mismatched = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_drop_index("prod", "main", "items", "by_em", false),
    );
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("by_em", "items").unique().hmac(&mismatched),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

// --------------------------------------------------------------------------
// drop_user / drop_role
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_user_requires_hmac() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_user("d", ddl::drop_user("bob"));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_role_requires_hmac() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_role("d", ddl::drop_role("admin"));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

// --------------------------------------------------------------------------
// grant_role / revoke_role (task #542 — the single most dangerous op class)
// --------------------------------------------------------------------------

async fn make_db_with_user(db: &str, username: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let shamir = Arc::new(shamir);
    let handler = ShamirDbHandler::new(shamir.clone());
    let session = root_session();
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_user(username),
    );
    let mut b = Batch::new();
    b.id(0);
    b.create_user("u", ddl::create_user(username, "s3cretpw").hmac(&tag));
    let req = execute_built(db, b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    // Fail fast with a useful message if fixture setup itself breaks.
    let _ = expect_batch_ok(res);
    shamir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_role_without_hmac_rejected() {
    let shamir = make_db_with_user("scratch", "alice").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.grant_role("g", ddl::grant_role("superuser", "alice"));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_role_with_wrong_hmac_rejected() {
    let shamir = make_db_with_user("scratch", "alice").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.grant_role(
        "g",
        ddl::grant_role("superuser", "alice").hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_role_with_correct_hmac_accepted() {
    let shamir = make_db_with_user("scratch", "alice").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_grant_role("superuser", "alice"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.grant_role("g", ddl::grant_role("superuser", "alice").hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["g"].records[0];
    assert_eq!(rec.get_value_str("granted_role"), Some("superuser"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_role_without_hmac_rejected() {
    let shamir = make_db_with_user("scratch", "alice").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.revoke_role("r", ddl::revoke_role("superuser", "alice"));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_role_with_wrong_hmac_rejected() {
    let shamir = make_db_with_user("scratch", "alice").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.revoke_role(
        "r",
        ddl::revoke_role("superuser", "alice").hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_role_with_correct_hmac_accepted() {
    let shamir = make_db_with_user("scratch", "alice").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_revoke_role("superuser", "alice"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.revoke_role("r", ddl::revoke_role("superuser", "alice").hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["r"].records[0];
    assert_eq!(rec.get_value_str("revoked_role"), Some("superuser"));
}

// --------------------------------------------------------------------------
// chmod / chown / chgrp
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chmod_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chmod("c", ddl::chmod(ddl::res::database("scratch"), 0o700));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chmod_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chmod(
        "c",
        ddl::chmod(ddl::res::database("scratch"), 0o700).hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chmod_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let resource = ddl::res::database("scratch");
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_chmod(&resource, 0o700),
    );
    let mut b = Batch::new();
    b.id(1);
    b.chmod("c", ddl::chmod(resource, 0o700).hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["c"].records[0];
    assert_eq!(rec.get_value_i64("mode"), Some(0o700));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chown_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chown("c", ddl::chown(ddl::res::database("scratch"), 7));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chown_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chown(
        "c",
        ddl::chown(ddl::res::database("scratch"), 7).hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chown_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let resource = ddl::res::database("scratch");
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_chown(&resource, 7),
    );
    let mut b = Batch::new();
    b.id(1);
    b.chown("c", ddl::chown(resource, 7).hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert!(!resp.results["c"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chgrp_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chgrp("c", ddl::chgrp(ddl::res::database("scratch"), Some(3)));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chgrp_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chgrp(
        "c",
        ddl::chgrp(ddl::res::database("scratch"), Some(3)).hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chgrp_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let resource = ddl::res::database("scratch");
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_chgrp(&resource, Some(3)),
    );
    let mut b = Batch::new();
    b.id(1);
    b.chgrp("c", ddl::chgrp(resource, Some(3)).hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert!(!resp.results["c"].records.is_empty());
}

// --------------------------------------------------------------------------
// create_user / create_role
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_user_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_user("u", ddl::create_user("bob", "s3cretpw"));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_user_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_user(
        "u",
        ddl::create_user("bob", "s3cretpw").hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_user_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let tag = canon::compute_tag_hex(&session_key(&session), &canon::canonical_create_user("bob"));
    let mut b = Batch::new();
    b.id(1);
    b.create_user("u", ddl::create_user("bob", "s3cretpw").hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["u"].records[0];
    assert_eq!(rec.get_value_str("created_user"), Some("bob"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_role_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_role("r", ddl::create_role("viewer", vec![]));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_role_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_role(
        "r",
        ddl::create_role("viewer", vec![]).hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_role_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_role("viewer"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.create_role("r", ddl::create_role("viewer", vec![]).hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["r"].records[0];
    assert_eq!(rec.get_value_str("created_role"), Some("viewer"));
}

// --------------------------------------------------------------------------
// set_retention / purge_history
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_retention_without_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.set_retention(
        "s",
        ddl::set_retention("items", ddl::Retention::current_only()).repo("main"),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_retention_with_wrong_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.set_retention(
        "s",
        ddl::set_retention("items", ddl::Retention::current_only())
            .repo("main")
            .hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_retention_with_correct_hmac_accepted() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let retention = ddl::Retention::current_only();
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_set_retention("prod", "main", "items", &retention),
    );
    let mut b = Batch::new();
    b.id(1);
    b.set_retention(
        "s",
        ddl::set_retention("items", retention)
            .repo("main")
            .hmac(&tag),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["s"].records[0];
    assert_eq!(rec.get_value_str("set_retention"), Some("items"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_history_without_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.purge_history(
        "p",
        ddl::purge_history("items", PurgeScope::OlderThanAge { age_secs: 0 }).repo("main"),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_history_with_wrong_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.purge_history(
        "p",
        ddl::purge_history("items", PurgeScope::OlderThanAge { age_secs: 0 })
            .repo("main")
            .hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_history_with_correct_hmac_accepted() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let scope = PurgeScope::OlderThanAge { age_secs: 0 };
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_purge_history("prod", "main", "items", &scope),
    );
    let mut b = Batch::new();
    b.id(1);
    b.purge_history(
        "p",
        ddl::purge_history("items", scope).repo("main").hmac(&tag),
    );
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    // Accepted (not an hmac_* rejection) — the op's own semantics (empty
    // table, nothing to purge) are irrelevant to the HMAC gate under test.
    match res {
        DbResponse::Error { code, .. } => {
            assert_ne!(code, "hmac_required");
            assert_ne!(code, "hmac_mismatch");
        }
        DbResponse::Batch { .. } => {}
        other => panic!("unexpected response: {:?}", other),
    }
}

// --------------------------------------------------------------------------
// Non-destructive ops untouched
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_op_passes_without_hmac() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.query("r", shamir_query_builder::Query::from("items"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert_eq!(resp.results["r"].records.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_table_passes_without_hmac() {
    // CREATE is non-destructive — gate must not apply.
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("prod").await;
    let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("prod", cfg).await.unwrap();
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_table("t", ddl::create_table("x").repo("main"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    let rec = &resp.results["t"].records[0];
    assert_eq!(rec.get_value_str("created_table"), Some("x"));
}

// --------------------------------------------------------------------------
// Mixed batch: HMAC failure stops the whole batch
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_batch_one_drop_missing_hmac_fails_whole_batch() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // The read op is harmless; the drop op is missing its HMAC.
    let mut b = Batch::new();
    b.id(1);
    b.query("r", Query::from("items"));
    b.drop_table("d", ddl::drop_table("items").repo("main"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, message) = expect_error(res);
    assert_eq!(code, "hmac_required");
    // Error mentions which alias was unsigned.
    assert!(message.contains("'d'"), "{}", message);
}

// --------------------------------------------------------------------------
// Different sessions get different keys
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tag_signed_with_other_session_key_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // Compute a tag using a DIFFERENT session_id (pretending we're
    // another session). The server uses `session.hmac_key()` which
    // derives from session.session_id == [0u8;32]; the attacker
    // session would have a different id (we simulate with [1u8;32]).
    let other_sid = [1u8; 32];
    let other_key = canon::derive_session_hmac_key(&other_sid);
    let tag = canon::compute_tag_hex(
        &other_key,
        &canon::canonical_drop_table("prod", "main", "items"),
    );

    let mut b = Batch::new();
    b.id(1);
    b.drop_table("d", ddl::drop_table("items").repo("main").hmac(&tag));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

// --------------------------------------------------------------------------
// create_group / drop_group / rename_group / add_group_member /
// remove_group_member (task #551 — group-mutating ops coverage)
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_group_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_group("g", ddl::create_group("devs"));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_group_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_group(
        "g",
        ddl::create_group("devs").hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_group_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.create_group("g", ddl::create_group("devs").hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert!(!resp.results["g"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_group_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_group(
        "d",
        ddl::drop_group(ddl::GroupRef::Name {
            name: "devs".to_string(),
        }),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_group_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_group(
        "d",
        ddl::drop_group(ddl::GroupRef::Name {
            name: "devs".to_string(),
        })
        .hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_group_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    // Seed the group so the drop resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(&session_key(&session), &canon::canonical_drop_group(&group));
    let mut b = Batch::new();
    b.id(1);
    b.drop_group("d", ddl::drop_group(group).hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert!(!resp.results["d"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_group_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.rename_group(
        "r",
        ddl::rename_group(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            "engineers",
        ),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_group_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.rename_group(
        "r",
        ddl::rename_group(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            "engineers",
        )
        .hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_group_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    // Seed the group so the rename resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_rename_group(&group, "engineers"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.rename_group("r", ddl::rename_group(group, "engineers").hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert!(!resp.results["r"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_group_member_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.add_group_member(
        "a",
        ddl::add_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        ),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_group_member_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.add_group_member(
        "a",
        ddl::add_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        )
        .hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_group_member_with_correct_hmac_accepted() {
    let shamir = make_db_with_user("scratch", "bob").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // Seed the group so the membership add resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_add_group_member(&group, 42),
    );
    let mut b = Batch::new();
    b.id(1);
    b.add_group_member("a", ddl::add_group_member(group, 42).hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert!(!resp.results["a"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_group_member_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member(
        "r",
        ddl::remove_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        ),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_group_member_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member(
        "r",
        ddl::remove_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        )
        .hmac("deadbeef".repeat(8)),
    );
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_group_member_with_correct_hmac_accepted() {
    let shamir = make_db_with_user("scratch", "bob").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // Seed the group so the membership removal resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_remove_group_member(&group, 42),
    );
    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member("r", ddl::remove_group_member(group, 42).hmac(&tag));
    let req = execute_built("scratch", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = expect_batch_ok(res);
    assert!(!resp.results["r"].records.is_empty());
}
