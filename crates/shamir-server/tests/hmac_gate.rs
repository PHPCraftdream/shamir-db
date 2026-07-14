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

use shamir_connect::common::kdf_params::KdfParams;
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
use shamir_server::db_handler::{AdminGlue, DbRequest, DbResponse, ShamirDbHandler};
use shamir_server::user_directory::FjallUserDirectory;
use tempfile::TempDir;

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

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Build a handler wired with `AdminGlue` (real `FjallUserDirectory`) — the
/// `DbRequest::CreateScramUser` handler needs this to run past the HMAC
/// gate and actually create the user (mirrors `set_superuser_wire.rs`'s
/// `build_handler`).
async fn build_handler_with_admin() -> ShamirDbHandler {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(FjallUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue {
            user_dir,
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    // Hold `tmp` alive for the test's lifetime by leaking it — mirrors
    // `set_superuser_wire.rs`'s fixture.
    std::mem::forget(tmp);
    handler
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
// drop_user
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

// --------------------------------------------------------------------------
// grant_role / revoke_role (task #542 — the single most dangerous op class)
// --------------------------------------------------------------------------

/// Build a fresh in-memory ShamirDb for HMAC-gate rejection tests. Task
/// #559: `create_user` now routes through `UserAdminPort` (returns
/// `not_supported` without one), so the old fixture that seeded a real
/// user via the wire path no longer works. The HMAC-rejection tests below
/// don't need a real user — the HMAC gate runs BEFORE the handler/port —
/// so a bare db suffices.
async fn make_db_with_user(db: &str, _username: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    Arc::new(shamir)
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

// Task #559: `grant_role_with_correct_hmac_accepted` and
// `revoke_role_with_correct_hmac_accepted` were removed — they tested the
// positive path, which now routes through `UserAdminPort` (needs a real
// directory wired, covered in `user_admin_port_wire.rs`). The HMAC
// *rejection* tests below remain valid because the HMAC gate runs BEFORE
// the handler/port is consulted.

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
    // Task #561 §2: chgrp now validates the group id unconditionally, so
    // target a real (created) group — the test's point is the HMAC gate, not
    // the group's existence.
    let gid = shamir.create_group("devs").await.unwrap();
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chgrp("c", ddl::chgrp(ddl::res::database("scratch"), Some(gid)));
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
    // Task #561 §2: chgrp now validates the group id unconditionally, so
    // target a real (created) group — the test's point is the HMAC gate, not
    // the group's existence.
    let gid = shamir.create_group("devs").await.unwrap();
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.chgrp(
        "c",
        ddl::chgrp(ddl::res::database("scratch"), Some(gid)).hmac("deadbeef".repeat(8)),
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
    // Task #561 §2: chgrp now validates the group id unconditionally, so
    // target a real (created) group — the test's point is the HMAC gate, not
    // the group's existence.
    let gid = shamir.create_group("devs").await.unwrap();
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let resource = ddl::res::database("scratch");
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_chgrp(&resource, Some(gid)),
    );
    let mut b = Batch::new();
    b.id(1);
    b.chgrp("c", ddl::chgrp(resource, Some(gid)).hmac(&tag));
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

// Task #559: `create_user_with_correct_hmac_accepted` was removed — it
// tested the positive path, which now routes through `UserAdminPort`
// (needs a real directory wired). All `create_role`/`drop_role` HMAC
// tests were removed because those BatchOp variants no longer exist.
// The `create_user_without_hmac_rejected` and
// `create_user_with_wrong_hmac_rejected` tests above remain valid (the
// HMAC gate runs before the handler/port).

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
    b.create_group("g", ddl::create_group("devs").hmac("deadbeef".repeat(8)));
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

// --------------------------------------------------------------------------
// create_scram_user (task #604 — top-level DbRequest, not a BatchOp, so it
// is gated inline in `create_scram_user`'s handler, mirroring
// `SetSuperuser`'s pattern).
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_without_hmac_rejected() {
    let handler = build_handler_with_admin().await;
    let session = root_session();

    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec!["user".into()],
        hmac: None,
    };
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
async fn create_scram_user_with_wrong_hmac_rejected() {
    let handler = build_handler_with_admin().await;
    let session = root_session();

    // Tag computed for a different name — must not validate for "bob".
    let wrong_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_scram_user("someone_else", &["user".to_string()]),
    );
    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec!["user".into()],
        hmac: Some(wrong_tag),
    };
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
async fn create_scram_user_with_correct_hmac_accepted() {
    let handler = build_handler_with_admin().await;
    let session = root_session();

    let roles = vec!["user".to_string()];
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_scram_user("bob", &roles),
    );
    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles,
        hmac: Some(tag),
    };
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
    match res {
        DbResponse::UserCreated { name, .. } => assert_eq!(name, "bob"),
        other => panic!("expected UserCreated, got {:?}", other),
    }
}

/// Task #605 — `create_scram_user` normalizes `name` through
/// `NormalizedUsername::from_raw` right after the HMAC gate, mirroring the
/// login path (`handshake.rs`). A PRECIS-invalid name (a NUL byte embedded
/// in the username, guaranteed rejected by `UsernameCaseMapped::enforce` —
/// see `crates/shamir-connect/src/common/tests/username_tests.rs`) must be
/// rejected with `code == "invalid_username"`, not persisted as a raw,
/// unnormalized account.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_with_invalid_precis_name_rejected() {
    let handler = build_handler_with_admin().await;
    let session = root_session();

    let invalid_name = "alice\0bob";
    let roles = vec!["user".to_string()];
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_scram_user(invalid_name, &roles),
    );
    let req = DbRequest::CreateScramUser {
        name: invalid_name.into(),
        password: "correct horse battery staple".into(),
        roles,
        hmac: Some(tag),
    };
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
    assert_eq!(code, "invalid_username");
}
