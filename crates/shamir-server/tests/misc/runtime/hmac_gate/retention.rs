use std::sync::Arc;

use crate::common::{
    decode, encode, execute_built, expect_batch_ok, expect_error, make_db_with_table, root_session,
    session_key,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::PurgeScope;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::{DbResponse, ShamirDbHandler};

// --------------------------------------------------------------------------
// create_user / create_role
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_user_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_user(
        "u",
        shamir_query_types::batch::BatchOp::CreateUser(shamir_query_types::auth::CreateUserOp {
            create_user: "bob".into(),
            password: "s3cretpw".into(),
            roles: Vec::new(),
            database: None,
            hmac: None,
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
async fn create_user_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_user(
        "u",
        ddl::create_user("bob", "s3cretpw")
            .hmac("deadbeef".repeat(8))
            .build()
            .expect("build HMAC test op"),
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
async fn create_user_with_roles_changed_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let op =
        shamir_query_types::batch::BatchOp::CreateUser(shamir_query_types::auth::CreateUserOp {
            create_user: "bob".into(),
            password: "s3cretpw".into(),
            roles: vec!["reader".into()],
            database: None,
            hmac: None,
        });
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("scratch", &op).expect("create-user canonical form"),
    );

    let mut b = Batch::new();
    b.id(1);
    b.create_user(
        "u",
        shamir_query_types::batch::BatchOp::CreateUser(shamir_query_types::auth::CreateUserOp {
            create_user: "bob".into(),
            password: "s3cretpw".into(),
            roles: vec!["admin".into()],
            database: None,
            hmac: Some(tag),
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
        shamir_query_types::batch::BatchOp::SetRetention(
            shamir_query_types::admin::SetRetentionOp {
                set_retention: "items".into(),
                repo: "main".into(),
                retention: ddl::Retention::current_only(),
                hmac: None,
            },
        ),
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
            .hmac("deadbeef".repeat(8))
            .build()
            .expect("build HMAC test op"),
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
    let op = shamir_query_types::batch::BatchOp::SetRetention(
        shamir_query_types::admin::SetRetentionOp {
            set_retention: "items".into(),
            repo: "main".into(),
            retention: retention.clone(),
            hmac: None,
        },
    );
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("prod", &op).expect("set-retention canonical form"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.set_retention(
        "s",
        ddl::set_retention("items", retention)
            .repo("main")
            .hmac(&tag)
            .build()
            .expect("build HMAC test op"),
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
        shamir_query_types::batch::BatchOp::PurgeHistory(
            shamir_query_types::admin::PurgeHistoryOp {
                purge_history: "items".into(),
                repo: "main".into(),
                scope: PurgeScope::OlderThanAge { age_secs: 0 },
                hmac: None,
            },
        ),
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
            .hmac("deadbeef".repeat(8))
            .build()
            .expect("build HMAC test op"),
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
    let op = shamir_query_types::batch::BatchOp::PurgeHistory(
        shamir_query_types::admin::PurgeHistoryOp {
            purge_history: "items".into(),
            repo: "main".into(),
            scope: scope.clone(),
            hmac: None,
        },
    );
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("prod", &op).expect("purge-history canonical form"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.purge_history(
        "p",
        ddl::purge_history("items", scope)
            .repo("main")
            .hmac(&tag)
            .build()
            .expect("build HMAC test op"),
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
