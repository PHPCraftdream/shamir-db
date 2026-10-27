//! `CreateScramUser` — wire-side SCRAM user creation: admin-glue presence,
//! permission gate ordering, success + duplicate, reserved-role atomicity.

use std::sync::Arc;

use tempfile::TempDir;

use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_db::ShamirDb;

use shamir_query_types::hmac as canon;
use shamir_server::db_handler::{AdminGlue, DbRequest, DbResponse, ShamirDbHandler};
use shamir_server::user_directory::FjallUserDirectory;

use crate::common::{decode, encode, root_session, user_session};

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_denied_without_admin_glue() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db)); // no admin glue
    let session = root_session();

    // Correct hmac so this test exercises the `not_supported` path (AdminGlue
    // missing), not the HMAC gate that now runs first (task #604).
    let roles = vec!["user".to_string()];
    let unsigned = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: roles.clone(),
        hmac: None,
    };
    let tag = canon::compute_tag_hex(
        &canon::derive_session_hmac_key(&session.session_id),
        &canon::canonical_db_request(&unsigned).expect("create-scram canonical form"),
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
        DbResponse::Error { code, .. } => assert_eq!(code, "not_supported"),
        other => panic!("expected not_supported, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_denied_for_non_superuser() {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(FjallUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue {
            user_dir: user_dir.clone(),
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    let session = user_session();

    // No hmac — proves the permission gate fires FIRST, before the HMAC
    // check (task #604, mirrors `set_superuser`'s ordering).
    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec![],
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
    match res {
        DbResponse::Error { code, .. } => assert_eq!(code, "permission_denied"),
        other => panic!("expected permission_denied, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_success_then_duplicate() {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(FjallUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue {
            user_dir: user_dir.clone(),
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    let session = root_session();

    let roles = vec!["read_write".to_string()];
    let unsigned = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: roles.clone(),
        hmac: None,
    };
    let tag = canon::compute_tag_hex(
        &canon::derive_session_hmac_key(&session.session_id),
        &canon::canonical_db_request(&unsigned).expect("create-scram canonical form"),
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
        DbResponse::UserCreated { name, user_id } => {
            assert_eq!(name, "bob");
            assert_eq!(user_id.len(), 16, "user_id is a stable 16-byte handle");
        }
        other => panic!("expected UserCreated, got {:?}", other),
    }

    use shamir_connect::server::admin::UserDirectory;
    assert!(
        user_dir.lookup_by_name("bob").is_some(),
        "persisted in directory"
    );
    let roles = user_dir.lookup_roles("bob").unwrap().unwrap_or_default();
    assert!(roles.iter().any(|r| r == "read_write"), "roles attached");

    // Second insert with same name -> typed user_exists error.
    let roles2: Vec<String> = vec![];
    let unsigned = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "another password".into(),
        roles: roles2.clone(),
        hmac: None,
    };
    let tag2 = canon::compute_tag_hex(
        &canon::derive_session_hmac_key(&session.session_id),
        &canon::canonical_db_request(&unsigned).expect("create-scram canonical form"),
    );
    let req2 = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "another password".into(),
        roles: roles2,
        hmac: Some(tag2),
    };
    let res2 = decode(
        &handler
            .handle(
                &session,
                &encode(&req2),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res2 {
        DbResponse::Error { code, .. } => assert_eq!(code, "user_exists"),
        other => panic!("expected user_exists, got {:?}", other),
    }
}

/// `CreateScramUser` with the reserved "superuser" role in its roles list
/// must fail atomically — no orphan, roleless account left behind. Before
/// this fix, `update_roles`'s reservation-rejection error was silently
/// discarded (`let _ = ...`) and the handler still reported `UserCreated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_with_reserved_superuser_role_leaves_no_orphan_account() {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(FjallUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue {
            user_dir: user_dir.clone(),
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    let session = root_session();

    let roles = vec!["superuser".to_string()];
    let unsigned = DbRequest::CreateScramUser {
        name: "frank".into(),
        password: "correct horse battery staple".into(),
        roles: roles.clone(),
        hmac: None,
    };
    let tag = canon::compute_tag_hex(
        &canon::derive_session_hmac_key(&session.session_id),
        &canon::canonical_db_request(&unsigned).expect("create-scram canonical form"),
    );
    let req = DbRequest::CreateScramUser {
        name: "frank".into(),
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
        DbResponse::Error { .. } => {}
        other => panic!("expected Error, got {:?}", other),
    }

    use shamir_connect::server::admin::UserDirectory;
    assert!(
        user_dir.lookup_by_name("frank").is_none(),
        "frank must NOT be persisted in the directory after create_scram_user \
         is rejected for requesting the reserved 'superuser' role"
    );
}
