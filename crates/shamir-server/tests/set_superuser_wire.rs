//! Wire-op integration tests for `DbRequest::SetSuperuser` (task #557).
//!
//! Drives the real `ShamirDbHandler` / `RequestHandler::handle` dispatch
//! (the same entry point a real connection uses) — NOT the directory
//! directly. This proves the full wire path:
//!
//!   - superuser session + correct HMAC → `SuperuserSet { user, on }`,
//!     directory state reflects the change;
//!   - missing HMAC → `hmac_required`;
//!   - wrong HMAC → `hmac_mismatch`;
//!   - non-superuser session → `permission_denied` (checked BEFORE the
//!     HMAC check, matching `create_scram_user`'s ordering);
//!   - revoking the last superuser (correct HMAC, superuser session) →
//!     typed refusal, not a silent success.
//!
//! The handler is built with `AdminGlue` (real `FjallUserDirectory`); the
//! session is constructed directly (no real SCRAM exchange — this test is
//! about `SetSuperuser` dispatch, not authentication, mirroring the
//! pattern in `db_handler.rs`'s `create_scram_user_*` tests).

use std::sync::Arc;

use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_connect::server::user_record::UserRecord;

use shamir_db::ShamirDb;
use shamir_query_types::hmac as canon;
use shamir_query_types::wire::{DbRequest, DbResponse};

use shamir_server::db_handler::{AdminGlue, ShamirDbHandler};
use shamir_server::user_directory::FjallUserDirectory;

use shamir_connect::common::crypto::StoredKey;
use tempfile::TempDir;
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn fixture_record() -> UserRecord {
    let salt = [0xa1u8; 16];
    let stored = StoredKey([0xc3u8; 32]);
    let mut server_key = Zeroizing::new([0u8; 32]);
    for (i, b) in server_key.iter_mut().enumerate() {
        *b = i as u8;
    }
    UserRecord {
        salt,
        stored_key: stored,
        server_key,
        kdf_params: KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

/// A superuser session (the caller). Constructed directly — what matters
/// for HMAC validation is that the test computes its tag with the SAME
/// `session_id` the server sees.
fn root_session() -> Session {
    Session::new(
        [0xAB; 16],
        "root".into(),
        SessionPermissions::from_roles(vec!["superuser".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

/// A non-superuser session for the `permission_denied` check.
fn user_session() -> Session {
    Session::new(
        [0xCD; 16],
        "carol".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

fn session_key(session: &Session) -> [u8; 32] {
    canon::derive_session_hmac_key(&session.session_id)
}

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

fn expect_error(res: DbResponse) -> (String, String) {
    match res {
        DbResponse::Error { code, message } => (code, message),
        other => panic!("expected Error, got {:?}", other),
    }
}

/// Build a handler wired with `AdminGlue` over a fresh `FjallUserDirectory`.
async fn build_handler() -> (ShamirDbHandler, Arc<FjallUserDirectory>) {
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
    // Hold `tmp` alive for the test's lifetime by leaking it — the tempdir
    // is small and tests are short-lived. (The alternative — returning the
    // `TempDir` — would require a fourth tuple element; the leak is the
    // simpler choice used elsewhere in this suite.)
    std::mem::forget(tmp);
    (handler, user_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: superuser session + correct HMAC grants superuser status on
/// a non-superuser target. Response is `SuperuserSet { user, on: true }`
/// and the directory reflects the change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_superuser_with_correct_hmac_grants() {
    let (handler, user_dir) = build_handler().await;
    let target_uid = user_dir
        .insert("bob".to_string(), fixture_record())
        .unwrap();
    assert!(
        !user_dir.state_by_user_id(&target_uid).unwrap().superuser,
        "precondition: bob starts as a non-superuser"
    );

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_set_superuser("bob", true));

    let req = DbRequest::SetSuperuser {
        user: "bob".into(),
        on: true,
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
        DbResponse::SuperuserSet { user, on } => {
            assert_eq!(user, "bob");
            assert!(on, "on must echo the requested value (true)");
        }
        other => panic!("expected SuperuserSet, got {:?}", other),
    }
    // Directory reflects the change.
    assert!(
        user_dir.state_by_user_id(&target_uid).unwrap().superuser,
        "bob's flag must be true after a successful SetSuperuser grant"
    );
}

/// Missing HMAC → `hmac_required` (after the superuser check passes).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_superuser_missing_hmac_rejected() {
    let (handler, _user_dir) = build_handler().await;
    let session = root_session();

    let req = DbRequest::SetSuperuser {
        user: "bob".into(),
        on: true,
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

/// Wrong HMAC → `hmac_mismatch`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_superuser_wrong_hmac_rejected() {
    let (handler, _user_dir) = build_handler().await;
    let session = root_session();

    let req = DbRequest::SetSuperuser {
        user: "bob".into(),
        on: true,
        hmac: Some("deadbeef".repeat(8)), // 64 hex chars but bogus
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

/// Non-superuser session → `permission_denied`, checked BEFORE the HMAC
/// gate (matching `create_scram_user`'s ordering — a missing/wrong hmac on
/// a non-superuser session still returns `permission_denied`, not
/// `hmac_required`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_superuser_non_superuser_session_denied() {
    let (handler, _user_dir) = build_handler().await;
    let session = user_session(); // non-superuser

    // Pass a (wrong) hmac to prove the permission check fires FIRST — if
    // it didn't, this would return `hmac_mismatch`, not `permission_denied`.
    let req = DbRequest::SetSuperuser {
        user: "bob".into(),
        on: true,
        hmac: Some("deadbeef".repeat(8)),
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
    assert_eq!(
        code, "permission_denied",
        "permission check must fire before the HMAC check"
    );
}

/// Revoking the last remaining superuser (correct HMAC, superuser session)
/// returns the typed refusal from `set_superuser`, not a silent success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_superuser_revoke_last_refused() {
    let (handler, user_dir) = build_handler().await;
    // The session's own user ("root") is the only superuser.
    let root_uid = user_dir
        .insert("root".to_string(), fixture_record())
        .unwrap();
    user_dir.set_superuser("root", true, 1_000).unwrap();
    assert!(
        user_dir.state_by_user_id(&root_uid).unwrap().superuser,
        "precondition: root is a superuser"
    );

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_set_superuser("root", false));

    let req = DbRequest::SetSuperuser {
        user: "root".into(),
        on: false,
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
    let (code, msg) = expect_error(res);
    assert!(
        msg.contains("last remaining superuser"),
        "wire error must surface the directory's typed refusal: {msg}"
    );
    // The error code is a codebase-conventional string (see task brief §6:
    // pick whatever fits from existing vocabulary). We assert it's not the
    // generic `query` fallback to prove the directory's specific refusal
    // was surfaced — the EXACT code is asserted in
    // `set_superuser_revoke_last_uses_invalid_owner_code` below.
    assert!(
        code != "query",
        "the last-superuser refusal must surface a typed code, not the generic `query` fallback"
    );

    // Flag stays true (refusal short-circuited before any write).
    assert!(
        user_dir.state_by_user_id(&root_uid).unwrap().superuser,
        "root's flag must stay true after a refused revoke"
    );
}

/// Pin the exact error code for the last-superuser refusal so a future
/// refactor that drifts to a less semantically meaningful code fails this
/// test loudly. See the task summary for the rationale: `invalid_owner`
/// matches `admin_access.rs`'s chown-to-OWNER_SYSTEM refusal precedent
/// (a privileged mutation refused on system-integrity grounds).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_superuser_revoke_last_uses_invalid_owner_code() {
    let (handler, user_dir) = build_handler().await;
    user_dir
        .insert("root".to_string(), fixture_record())
        .unwrap();
    user_dir.set_superuser("root", true, 1_000).unwrap();

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_set_superuser("root", false));

    let req = DbRequest::SetSuperuser {
        user: "root".into(),
        on: false,
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
    assert_eq!(
        code, "invalid_owner",
        "last-superuser refusal must use `invalid_owner` (matches admin_access.rs's \
         chown-to-OWNER_SYSTEM refusal precedent for a privileged mutation refused \
         on system-integrity grounds)"
    );
}

/// Target user doesn't exist → `not_found` (matches the codebase's existing
/// `not_found` convention for "target entity doesn't exist" — GrantRole,
/// admin_describe, admin_replication, etc.).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_superuser_unknown_user_returns_not_found() {
    let (handler, _user_dir) = build_handler().await;
    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_set_superuser("ghost", true));

    let req = DbRequest::SetSuperuser {
        user: "ghost".into(),
        on: true,
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
    let (code, msg) = expect_error(res);
    assert_eq!(
        code, "not_found",
        "target user doesn't exist must surface `not_found` (matches GrantRole precedent): {msg}"
    );
}
