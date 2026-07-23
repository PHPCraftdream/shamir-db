//! CR-A6 regression: `changePassword` must still work on a record that was
//! just rotated by [`crate::bootstrap::rotate_bootstrap_credential_to_random`]
//! (the helper the production first-login/TTL-sweep paths call) — proving
//! the credential rotation CR-A6 adds doesn't somehow break the normal
//! changePassword ceremony that would follow a real client's first token
//! login.
//!
//! Lives in `src/tests/` (not `tests/`) because
//! `rotate_bootstrap_credential_to_random` is `pub(crate)` — an
//! internal helper shared between `connection/handshake.rs` and
//! `server/server_launcher.rs`, not part of the public API a `tests/*.rs`
//! integration test could reach.
//!
//! Mirrors the dispatch-level harness in `tests/change_password_e2e.rs`
//! (build a handler + session directly, round-trip `DbRequest`s through
//! `dispatch_request_view` — the same production entry point a real
//! TCP/WS connection uses) rather than spinning up a real listener.

use std::sync::Arc;

use shamir_connect::common::changepw::{build_auth_message_cp, ChangePwAuthMessageInputs};
use shamir_connect::common::crypto::hmac_sha256;
use shamir_connect::common::envelope::{RequestEnvelope, RequestEnvelopeView};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{limits, BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::{dispatch_request_view, DispatchOutcome};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_connect::server::user_record::UserRecord;

use shamir_db::ShamirDb;

use shamir_query_types::wire::{DbRequest, DbResponse};

use zeroize::Zeroizing;

use crate::bootstrap::rotate_bootstrap_credential_to_random;
use crate::db_handler::{AdminGlue, ShamirDbHandler};
use crate::user_directory::FjallUserDirectory;

use tempfile::TempDir;

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

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

fn register_user(
    user_dir: &FjallUserDirectory,
    username: &str,
    password: &[u8],
    salt: [u8; 16],
    kdf: KdfParams,
) -> [u8; 16] {
    let derived = DerivedKeys::derive(password, &salt, &kdf).expect("derive");
    let record = UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: Zeroizing::new(*derived.server_key),
        kdf_params: kdf,
        tickets_invalid_before_ns: 0,
    };
    user_dir
        .insert(username.to_string(), record)
        .expect("insert user")
}

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

/// The rotated password itself is unknowable by design (that's the whole
/// point of CR-A6), so this test builds the changePassword "old-password"
/// proof directly from the persisted post-rotation `stored_key` — exactly
/// what `verify_change_password_request_with_sid` itself checks against.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_password_still_works_after_bootstrap_rotation() {
    let (handler, store, user_dir) = build_handler_and_store().await;
    let kdf = fast_kdf();

    let user_id = register_user(&user_dir, "admin", b"the-bootstrap-token", [0xAA; 16], kdf);
    let now_ns = UnixNanos::now().as_u64();
    rotate_bootstrap_credential_to_random(&user_dir, "admin", kdf, now_ns)
        .await
        .expect("rotation must succeed");
    let rotated = user_dir
        .lookup_by_name("admin")
        .expect("admin still exists after rotation");

    // Standing in for the session a real client still holds immediately
    // after its first (token) login — rotation swaps the credential but
    // does not kill the session that just authenticated.
    let sid_a = insert_session(&store, user_id, "admin", 0xA1);

    // Step 1: challenge — must echo the ROTATED salt.
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
    let (server_nonce_cp, resp_salt) = match challenge_resp {
        DbResponse::ChangePasswordChallenge {
            server_nonce_cp,
            salt,
            ..
        } => (server_nonce_cp, salt),
        other => panic!("expected ChangePasswordChallenge, got {:?}", other),
    };
    assert_eq!(
        resp_salt,
        rotated.salt.to_vec(),
        "challenge must echo the ROTATED salt"
    );

    // Step 2: a wrong proof must still be rejected cleanly against the
    // ROTATED record (proves verify reads the rotated stored_key, not a
    // stale pre-rotation one).
    let server_nonce_arr: [u8; 32] = server_nonce_cp.try_into().unwrap();
    let client_nonce_arr: [u8; 32] = client_nonce_cp.try_into().unwrap();
    let auth_message_cp = build_auth_message_cp(ChangePwAuthMessageInputs {
        username: &NormalizedUsername::from_normalized_unchecked("admin".to_string()),
        session_id: &sid_a,
        client_nonce_cp: &client_nonce_arr,
        server_nonce_cp: &server_nonce_arr,
        salt: &rotated.salt,
        kdf_params: kdf,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        channel_binding_at_auth: &[0u8; 32],
    })
    .expect("auth_message_cp");
    let wrong_client_key = [0x77u8; 32];
    let signature = hmac_sha256(&rotated.stored_key.0, &auth_message_cp);
    let mut bad_proof = [0u8; 32];
    for i in 0..32 {
        bad_proof[i] = wrong_client_key[i] ^ signature[i];
    }
    let new_salt: [u8; 16] = [0xDD; 16];
    let new_derived = DerivedKeys::derive(b"brand new password", &new_salt, &kdf).unwrap();
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
    assert_eq!(
        user_dir.lookup_by_name("admin").unwrap().salt,
        rotated.salt,
        "a rejected changePassword must not touch the rotated record"
    );

    // Step 3: drive the ACCEPT path for real. Seed a KNOWN password via
    // `update_credentials` (the exact primitive rotation and changePassword
    // both use) to stand in for "the operator now knows this session's real
    // password", then run the full challenge->verify ceremony — proving
    // changePassword's happy path is unaffected by a prior rotation.
    let known_password = b"known-password-after-bootstrap-rotation";
    let known_salt: [u8; 16] = [0xCC; 16];
    let known_derived = DerivedKeys::derive(known_password, &known_salt, &kdf).unwrap();
    user_dir
        .update_credentials(
            "admin",
            known_salt,
            known_derived.stored_key,
            *known_derived.server_key,
            kdf,
            now_ns,
        )
        .expect("seed a known password to drive the accept path");

    let client_nonce_cp2 = vec![0x44u8; 32];
    let challenge_resp2 = dispatch(
        &handler,
        &store,
        &sid_a,
        &DbRequest::ChangePasswordChallenge {
            client_nonce_cp: client_nonce_cp2.clone(),
        },
        0,
    )
    .await;
    let server_nonce_cp2 = match challenge_resp2 {
        DbResponse::ChangePasswordChallenge {
            server_nonce_cp, ..
        } => server_nonce_cp,
        other => panic!("expected ChangePasswordChallenge, got {:?}", other),
    };
    let server_nonce_arr2: [u8; 32] = server_nonce_cp2.try_into().unwrap();
    let client_nonce_arr2: [u8; 32] = client_nonce_cp2.try_into().unwrap();
    let auth_message_cp2 = build_auth_message_cp(ChangePwAuthMessageInputs {
        username: &NormalizedUsername::from_normalized_unchecked("admin".to_string()),
        session_id: &sid_a,
        client_nonce_cp: &client_nonce_arr2,
        server_nonce_cp: &server_nonce_arr2,
        salt: &known_salt,
        kdf_params: kdf,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        channel_binding_at_auth: &[0u8; 32],
    })
    .expect("auth_message_cp");
    let signature2 = hmac_sha256(&known_derived.stored_key.0, &auth_message_cp2);
    let mut client_proof_old = [0u8; 32];
    for i in 0..32 {
        client_proof_old[i] = known_derived.client_key[i] ^ signature2[i];
    }
    let final_salt: [u8; 16] = [0xEE; 16];
    let final_derived = DerivedKeys::derive(b"final new password", &final_salt, &kdf).unwrap();
    let verify_resp2 = dispatch(
        &handler,
        &store,
        &sid_a,
        &DbRequest::ChangePasswordVerify {
            client_proof_old: client_proof_old.to_vec(),
            new_salt: final_salt.to_vec(),
            new_stored_key: final_derived.stored_key.0.to_vec(),
            new_server_key: final_derived.server_key.to_vec(),
        },
        0,
    )
    .await;
    assert!(
        matches!(verify_resp2, DbResponse::ChangePasswordOk),
        "changePassword must succeed on a record that was previously rotated, got {:?}",
        verify_resp2
    );
    let record = user_dir
        .lookup_by_name("admin")
        .expect("admin still exists");
    assert_eq!(
        record.salt, final_salt,
        "changePassword's new credentials must persist normally after a prior rotation"
    );
}
