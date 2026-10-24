use std::sync::Arc;

use crate::common::{
    build_handler_with_admin, decode, encode, execute_built, expect_batch_ok, expect_error,
    make_db_with_table, root_session, session_key,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::Query;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

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
    shamir.create_db("prod").await.expect("create_db");
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
