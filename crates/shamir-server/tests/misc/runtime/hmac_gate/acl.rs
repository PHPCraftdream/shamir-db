// --------------------------------------------------------------------------
// grant_role / revoke_role (task #542 — the single most dangerous op class)
// --------------------------------------------------------------------------

use std::sync::Arc;

use crate::common::{
    decode, encode, execute_built, expect_batch_ok, expect_error, root_session, session_key,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::ShamirDbHandler;

/// Build a fresh in-memory ShamirDb for HMAC-gate rejection tests. Task
/// #559: `create_user` now routes through `UserAdminPort` (returns
/// `not_supported` without one), so the old fixture that seeded a real
/// user via the wire path no longer works. The HMAC-rejection tests below
/// don't need a real user — the HMAC gate runs BEFORE the handler/port —
/// so a bare db suffices.
pub(crate) async fn make_db_with_user(db: &str, _username: &str) -> Arc<ShamirDb> {
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
