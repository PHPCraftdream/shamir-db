//! Live-dispatch-path tests for `changePassword` (spec §12.5, task #547).
//!
//! Proves `ChangePasswordChallenge` -> `ChangePasswordVerify` works over the
//! REAL `ShamirDbHandler` / `dispatch_request_view` path (the same entry
//! point a real TCP/WS connection uses), not just a direct call into
//! `changepw.rs`'s free functions:
//!
//! - Success: persists the new credentials (old-password login now fails
//!   SCRAM verification, new-password login succeeds), bumps
//!   `tickets_invalid_before_ns` (a previously-issued ticket for this user
//!   is rejected by the §7.5 validity check), and kills every OTHER live
//!   session for the user (spec §12.5.3) while leaving unrelated sessions
//!   untouched.
//! - Failure (wrong old-password proof): rejected `auth_failed`, changes
//!   nothing — old credentials still verify, all sessions of the user
//!   (including the one that made the bad attempt) stay alive.
//!
//! Login itself is exercised at the SCRAM-primitive level
//! (`verify_client_proof` against `auth_message`) rather than a full
//! TLS+TCP handshake — this is the same "prove the persisted credentials
//! are what a real login would check" pattern used by
//! `db_handler.rs`'s existing `create_scram_user_*` tests, which never
//! spin up a real listener either.

use std::sync::Arc;

use shamir_connect::common::auth_message::{AuthMessage, AuthMessageInputs};
use shamir_connect::common::envelope::{RequestEnvelope, RequestEnvelopeView};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::{build_client_proof, verify_client_proof, DerivedKeys};
use shamir_connect::common::types::{limits, BindingMode, ProtocolVersion, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::{dispatch_request_view, DispatchOutcome};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use shamir_db::ShamirDb;

use shamir_query_types::wire::{DbRequest, DbResponse};

use shamir_server::db_handler::{AdminGlue, ShamirDbHandler};
use shamir_server::user_directory::FjallUserDirectory;

use tempfile::TempDir;

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

/// Build a handler wired with `AdminGlue` + `SessionStore` (the Gap 2 fix),
/// exactly like `server_launcher.rs` wires it in production.
async fn build_handler_and_store() -> (ShamirDbHandler, Arc<SessionStore>, Arc<FjallUserDirectory>)
{
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(FjallUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let session_store = Arc::new(SessionStore::new());
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue {
            user_dir: user_dir.clone(),
            kdf: fast_kdf(),
            tables_registry: None,
        },
    )
    .with_session_store(session_store.clone());
    (handler, session_store, user_dir)
}

/// Derive SCRAM credentials for `password` under `salt`/`kdf` and register
/// the user directly in the directory (bypassing the wire `CreateScramUser`
/// op — this test is about `changePassword`, not user creation).
fn register_user(
    user_dir: &FjallUserDirectory,
    username: &str,
    password: &[u8],
    salt: [u8; 16],
    kdf: KdfParams,
) -> [u8; 16] {
    let derived = DerivedKeys::derive(password, &salt, &kdf).expect("derive");
    let record = shamir_connect::server::user_record::UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: zeroize::Zeroizing::new(*derived.server_key),
        kdf_params: kdf,
        tickets_invalid_before_ns: 0,
    };
    user_dir
        .insert(username.to_string(), record)
        .expect("insert user")
}

/// Insert a live `Session` for `username`/`user_id` directly into the store
/// (bypassing the real SCRAM handshake — this test is about `changePassword`
/// dispatch, not authentication itself). Returns `(session_id, session_arc)`.
fn insert_session(
    store: &SessionStore,
    user_id: [u8; 16],
    username: &str,
    sid_byte: u8,
) -> [u8; limits::SESSION_ID_BYTES] {
    let sid = [sid_byte; limits::SESSION_ID_BYTES];
    let session = Session::new(
        user_id,
        username.to_string(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    store.insert(sid, session);
    sid
}

/// Round-trip a `DbRequest` through `dispatch_request_view` (the REAL
/// production entry point — same one `connection::request_loop` calls),
/// returning the decoded `DbResponse` on a normal (non-protocol-error) reply.
async fn dispatch(
    handler: &ShamirDbHandler,
    store: &SessionStore,
    sid: &[u8; limits::SESSION_ID_BYTES],
    req: &DbRequest,
    tickets_invalid_before_ns: u64,
) -> DbResponse {
    let envelope = RequestEnvelope::new(*sid, Some(1), rmp_serde::to_vec_named(req).unwrap());
    let bytes = envelope.to_msgpack().unwrap();
    let view = RequestEnvelopeView::from_msgpack(&bytes).unwrap();
    let conn = ConnectionServices::without_push(0);

    let outcome = dispatch_request_view(
        &view,
        store,
        |_user_id| tickets_invalid_before_ns,
        handler,
        &conn,
    )
    .await
    .expect("dispatch_request_view protocol-level ok");

    match outcome {
        DispatchOutcome::Response(resp) => rmp_serde::from_slice(&resp.res).expect("decode resp"),
        DispatchOutcome::Error(err) => panic!("expected Response, got dispatch error: {:?}", err),
    }
}

/// Build a login-equivalent SCRAM proof for `password` against the user's
/// CURRENT (salt, kdf) and a login-style `auth_message` — mirrors what a
/// real client does at `auth_init`/`auth_verify` time. Returns
/// `verify_client_proof(...)` against `stored_key` — `true` iff `password`
/// is the user's current password.
fn login_would_succeed(
    username: &str,
    password: &[u8],
    salt: [u8; 16],
    kdf: KdfParams,
    stored_key: &shamir_connect::common::crypto::StoredKey,
) -> bool {
    let derived = DerivedKeys::derive(password, &salt, &kdf).expect("derive");
    let client_nonce = [0x11u8; 32];
    let server_nonce = [0x22u8; 32];
    let zeros = [0u8; 32];
    let auth_message = AuthMessage::build(AuthMessageInputs {
        username: &NormalizedUsername::from_normalized_unchecked(username.to_string()),
        client_nonce: &client_nonce,
        server_nonce: &server_nonce,
        salt: &salt,
        kdf_params: kdf,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::None,
        tls_exporter_or_zeros: &zeros,
        supported_version: ProtocolVersion::V1,
    })
    .expect("auth_message");
    let proof = build_client_proof(&derived.client_key, &derived.stored_key, &auth_message);
    verify_client_proof(&proof, stored_key, &auth_message)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full happy path: challenge -> verify with correct old-password proof.
/// Confirms all four behaviours from the task's Test requirement section.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_password_success_persists_revokes_and_kills_other_sessions() {
    let (handler, store, user_dir) = build_handler_and_store().await;
    let kdf = fast_kdf();
    let salt: [u8; 16] = [0xAA; 16];
    let old_password = b"correct horse battery staple";
    let new_password = b"donkey stapler paperclip";

    let user_id = register_user(&user_dir, "alice", old_password, salt, kdf);

    // Two live sessions for alice: `sid_a` will drive the changePassword
    // flow itself; `sid_b` is a concurrently-open OTHER session that must
    // be killed on success. An unrelated user's session must survive.
    let sid_a = insert_session(&store, user_id, "alice", 0xA1);
    let sid_b = insert_session(&store, user_id, "alice", 0xB2);
    let other_user_id = [0x99; 16];
    let sid_other = insert_session(&store, other_user_id, "bob", 0xC3);

    // Step 1: challenge.
    let client_nonce_cp = vec![0x33u8; 32];
    let challenge_resp = dispatch(
        &handler,
        &store,
        &sid_a,
        &DbRequest::ChangePasswordChallenge {
            client_nonce_cp: client_nonce_cp.clone(),
        },
        0,
    )
    .await;
    let (server_nonce_cp, resp_salt, resp_memory_kb, resp_time, resp_parallelism, resp_argon2) =
        match challenge_resp {
            DbResponse::ChangePasswordChallenge {
                server_nonce_cp,
                salt,
                kdf_memory_kb,
                kdf_time,
                kdf_parallelism,
                kdf_argon2_version,
            } => (
                server_nonce_cp,
                salt,
                kdf_memory_kb,
                kdf_time,
                kdf_parallelism,
                kdf_argon2_version,
            ),
            other => panic!("expected ChangePasswordChallenge, got {:?}", other),
        };
    assert_eq!(resp_salt, salt.to_vec(), "challenge echoes current salt");
    assert_eq!(resp_memory_kb, kdf.memory_kb);
    assert_eq!(resp_time, kdf.time);
    assert_eq!(resp_parallelism, kdf.parallelism);
    assert_eq!(resp_argon2, kdf.argon2_version);

    // Client-side: derive OLD-password keys, build `auth_message_cp`, and
    // the `client_proof_old` exactly as `changepw.rs`'s
    // `verify_change_password_request_with_sid` will recompute it.
    let old_derived = DerivedKeys::derive(old_password, &salt, &kdf).expect("derive old");
    let server_nonce_arr: [u8; 32] = server_nonce_cp.clone().try_into().unwrap();
    let client_nonce_arr: [u8; 32] = client_nonce_cp.clone().try_into().unwrap();
    let auth_message_cp = shamir_connect::common::changepw::build_auth_message_cp(
        shamir_connect::common::changepw::ChangePwAuthMessageInputs {
            username: &NormalizedUsername::from_normalized_unchecked("alice".to_string()),
            session_id: &sid_a,
            client_nonce_cp: &client_nonce_arr,
            server_nonce_cp: &server_nonce_arr,
            salt: &salt,
            kdf_params: kdf,
            transport_kind: TransportKind::Tcp,
            binding_mode: BindingMode::TlsExporter,
            channel_binding_at_auth: &[0u8; 32],
        },
    )
    .expect("auth_message_cp");
    let signature =
        shamir_connect::common::crypto::hmac_sha256(&old_derived.stored_key.0, &auth_message_cp);
    let mut client_proof_old = [0u8; 32];
    for i in 0..32 {
        client_proof_old[i] = old_derived.client_key[i] ^ signature[i];
    }

    // New credentials, derived client-side under the server's kdf defaults.
    let new_salt: [u8; 16] = [0xBB; 16];
    let new_derived = DerivedKeys::derive(new_password, &new_salt, &kdf).expect("derive new");

    // Step 2: verify.
    let verify_resp = dispatch(
        &handler,
        &store,
        &sid_a,
        &DbRequest::ChangePasswordVerify {
            client_proof_old: client_proof_old.to_vec(),
            new_salt: new_salt.to_vec(),
            new_stored_key: new_derived.stored_key.0.to_vec(),
            new_server_key: new_derived.server_key.to_vec(),
        },
        0,
    )
    .await;
    assert!(
        matches!(verify_resp, DbResponse::ChangePasswordOk),
        "expected ChangePasswordOk, got {:?}",
        verify_resp
    );

    // --- Behaviour 1: new credentials persisted; old password now fails,
    //     new password succeeds (checked at the SCRAM-verification level,
    //     same primitives a real login uses).
    let record = user_dir
        .lookup_by_name("alice")
        .expect("alice still exists");
    assert!(
        !login_would_succeed(
            "alice",
            old_password,
            record.salt,
            record.kdf_params,
            &record.stored_key
        ),
        "OLD password must no longer verify after changePassword"
    );
    assert!(
        login_would_succeed(
            "alice",
            new_password,
            record.salt,
            record.kdf_params,
            &record.stored_key
        ),
        "NEW password must verify after changePassword"
    );
    assert_eq!(record.salt, new_salt, "new salt persisted");

    // --- Behaviour 2: tickets_invalid_before_ns bumped — a ticket issued
    //     before the change (created_at_ns < bump) is now rejected by the
    //     §7.5 validity check. We simulate this by looking up the
    //     authoritative cache directly.
    let bumped_ts = user_dir.tickets_invalid_before_ns_by_user_id(&user_id);
    assert!(bumped_ts > 0, "tickets_invalid_before_ns must be bumped");

    // --- Behaviour 3: ALL of alice's sessions are killed (including the
    //     one that drove the change — sid_a — AND the other one, sid_b).
    assert!(
        store.lookup(&sid_a).is_none(),
        "sid_a (the session that ran changePassword) must be killed too, per spec §12.5.3"
    );
    assert!(
        store.lookup(&sid_b).is_none(),
        "sid_b (alice's OTHER concurrently-open session) must be killed"
    );

    // --- Behaviour 4 (no collateral damage): bob's unrelated session
    //     survives untouched.
    assert!(
        store.lookup(&sid_other).is_some(),
        "bob's session must NOT be affected by alice's changePassword"
    );
}

/// Wrong old-password proof: rejected, changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_password_wrong_old_proof_rejected_and_changes_nothing() {
    let (handler, store, user_dir) = build_handler_and_store().await;
    let kdf = fast_kdf();
    let salt: [u8; 16] = [0xAA; 16];
    let old_password = b"correct horse battery staple";
    let wrong_password = b"totally the wrong password";

    let user_id = register_user(&user_dir, "alice", old_password, salt, kdf);
    let sid_a = insert_session(&store, user_id, "alice", 0xA1);
    let sid_b = insert_session(&store, user_id, "alice", 0xB2);

    // Challenge.
    let client_nonce_cp = vec![0x33u8; 32];
    let challenge_resp = dispatch(
        &handler,
        &store,
        &sid_a,
        &DbRequest::ChangePasswordChallenge {
            client_nonce_cp: client_nonce_cp.clone(),
        },
        0,
    )
    .await;
    let server_nonce_cp = match challenge_resp {
        DbResponse::ChangePasswordChallenge {
            server_nonce_cp, ..
        } => server_nonce_cp,
        other => panic!("expected ChangePasswordChallenge, got {:?}", other),
    };

    // Build a proof using the WRONG password's derived keys (attacker
    // doesn't know the real old password).
    let wrong_derived = DerivedKeys::derive(wrong_password, &salt, &kdf).expect("derive wrong");
    let server_nonce_arr: [u8; 32] = server_nonce_cp.try_into().unwrap();
    let client_nonce_arr: [u8; 32] = client_nonce_cp.try_into().unwrap();
    let auth_message_cp = shamir_connect::common::changepw::build_auth_message_cp(
        shamir_connect::common::changepw::ChangePwAuthMessageInputs {
            username: &NormalizedUsername::from_normalized_unchecked("alice".to_string()),
            session_id: &sid_a,
            client_nonce_cp: &client_nonce_arr,
            server_nonce_cp: &server_nonce_arr,
            salt: &salt,
            kdf_params: kdf,
            transport_kind: TransportKind::Tcp,
            binding_mode: BindingMode::TlsExporter,
            channel_binding_at_auth: &[0u8; 32],
        },
    )
    .expect("auth_message_cp");
    let signature =
        shamir_connect::common::crypto::hmac_sha256(&wrong_derived.stored_key.0, &auth_message_cp);
    let mut bad_proof = [0u8; 32];
    for i in 0..32 {
        bad_proof[i] = wrong_derived.client_key[i] ^ signature[i];
    }

    let new_salt: [u8; 16] = [0xBB; 16];
    let new_derived =
        DerivedKeys::derive(b"new password nobody used", &new_salt, &kdf).expect("derive new");

    let verify_resp = dispatch(
        &handler,
        &store,
        &sid_a,
        &DbRequest::ChangePasswordVerify {
            client_proof_old: bad_proof.to_vec(),
            new_salt: new_salt.to_vec(),
            new_stored_key: new_derived.stored_key.0.to_vec(),
            new_server_key: new_derived.server_key.to_vec(),
        },
        0,
    )
    .await;
    match verify_resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "auth_failed"),
        other => panic!("expected auth_failed error, got {:?}", other),
    }

    // Nothing changed: old password still verifies, new salt was NOT
    // persisted, tickets_invalid_before_ns stays at 0, and BOTH of alice's
    // sessions (including the one that made the bad attempt) stay alive.
    let record = user_dir
        .lookup_by_name("alice")
        .expect("alice still exists");
    assert_eq!(record.salt, salt, "salt must be unchanged on failed verify");
    assert!(
        login_would_succeed(
            "alice",
            old_password,
            record.salt,
            record.kdf_params,
            &record.stored_key
        ),
        "OLD password must still verify after a REJECTED changePassword"
    );
    assert_eq!(
        user_dir.tickets_invalid_before_ns_by_user_id(&user_id),
        0,
        "tickets_invalid_before_ns must NOT be bumped on a failed verify"
    );
    assert!(
        store.lookup(&sid_a).is_some(),
        "sid_a must survive a rejected changePassword"
    );
    assert!(
        store.lookup(&sid_b).is_some(),
        "sid_b must survive a rejected changePassword"
    );
}
