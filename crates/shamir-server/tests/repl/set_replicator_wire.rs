//! Wire-op integration tests for `DbRequest::SetReplicator` (task #621).
//!
//! Mirrors `set_superuser_wire.rs`'s pattern (drives the real
//! `ShamirDbHandler` / `RequestHandler::handle` dispatch, NOT the directory
//! directly) but WITHOUT the last-remaining-account specific cases — there
//! is no last-remaining guard for `replicator` (zero replicators is a
//! normal state).
//!
//! Covers:
//!   - superuser session + correct HMAC → `ReplicatorSet { user, on }`,
//!     directory state reflects the change;
//!   - missing HMAC → `hmac_required`;
//!   - wrong HMAC → `hmac_mismatch`;
//!   - non-superuser session → `permission_denied` (checked BEFORE the
//!     HMAC check, matching `set_superuser`'s ordering);
//!   - target user doesn't exist → `not_found`.

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

/// A superuser session (the caller).
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
    // Hold `tmp` alive for the test's lifetime by leaking it — mirrors
    // `set_superuser_wire.rs`'s established convention.
    std::mem::forget(tmp);
    (handler, user_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: superuser session + correct HMAC grants replicator status on
/// a non-replicator target. Response is `ReplicatorSet { user, on: true }`
/// and the directory reflects the change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_replicator_with_correct_hmac_grants() {
    let (handler, user_dir) = build_handler().await;
    let target_uid = user_dir
        .insert("bob".to_string(), fixture_record())
        .unwrap();
    assert!(
        !user_dir.state_by_user_id(&target_uid).unwrap().replicator,
        "precondition: bob starts as a non-replicator"
    );

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_set_replicator("bob", true));

    let req = DbRequest::SetReplicator {
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
        DbResponse::ReplicatorSet { user, on } => {
            assert_eq!(user, "bob");
            assert!(on, "on must echo the requested value (true)");
        }
        other => panic!("expected ReplicatorSet, got {:?}", other),
    }
    // Directory reflects the change.
    assert!(
        user_dir.state_by_user_id(&target_uid).unwrap().replicator,
        "bob's flag must be true after a successful SetReplicator grant"
    );
}

/// Missing HMAC → `hmac_required` (after the superuser check passes).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_replicator_missing_hmac_rejected() {
    let (handler, _user_dir) = build_handler().await;
    let session = root_session();

    let req = DbRequest::SetReplicator {
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
async fn set_replicator_wrong_hmac_rejected() {
    let (handler, _user_dir) = build_handler().await;
    let session = root_session();

    let req = DbRequest::SetReplicator {
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
/// gate (matching `set_superuser`'s ordering).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_replicator_non_superuser_session_denied() {
    let (handler, _user_dir) = build_handler().await;
    let session = user_session(); // non-superuser

    // Pass a (wrong) hmac to prove the permission check fires FIRST — if
    // it didn't, this would return `hmac_mismatch`, not `permission_denied`.
    let req = DbRequest::SetReplicator {
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

/// Target user doesn't exist → `not_found`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_replicator_unknown_user_returns_not_found() {
    let (handler, _user_dir) = build_handler().await;
    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_set_replicator("ghost", true));

    let req = DbRequest::SetReplicator {
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
        "target user doesn't exist must surface `not_found`: {msg}"
    );
}

/// Revoke path works too (grant then revoke), and a no-op re-grant is
/// idempotent (mirrors `set_superuser`'s idempotency contract, minus the
/// last-remaining specifics which don't apply here).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_replicator_revoke_after_grant_succeeds() {
    let (handler, user_dir) = build_handler().await;
    let target_uid = user_dir
        .insert("bob".to_string(), fixture_record())
        .unwrap();

    let session = root_session();
    let key = session_key(&session);

    // Grant.
    let grant_tag = canon::compute_tag_hex(&key, &canon::canonical_set_replicator("bob", true));
    let grant_req = DbRequest::SetReplicator {
        user: "bob".into(),
        on: true,
        hmac: Some(grant_tag),
    };
    let _ = decode(
        &handler
            .handle(
                &session,
                &encode(&grant_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    assert!(user_dir.state_by_user_id(&target_uid).unwrap().replicator);

    // Revoke — zero replicators afterward is a perfectly normal state
    // (no last-remaining guard for `replicator`).
    let revoke_tag = canon::compute_tag_hex(&key, &canon::canonical_set_replicator("bob", false));
    let revoke_req = DbRequest::SetReplicator {
        user: "bob".into(),
        on: false,
        hmac: Some(revoke_tag),
    };
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&revoke_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res {
        DbResponse::ReplicatorSet { user, on } => {
            assert_eq!(user, "bob");
            assert!(!on);
        }
        other => panic!("expected ReplicatorSet, got {:?}", other),
    }
    assert!(
        !user_dir.state_by_user_id(&target_uid).unwrap().replicator,
        "revoking the only replicator must succeed — no last-remaining guard"
    );
}
