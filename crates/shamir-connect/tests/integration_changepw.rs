//! Integration tests for `changePassword` (spec §12.5).

use shamir_connect::client::changepw as client_cp;
use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::changepw::{
    finalize_change_password, start_change_password_challenge,
    verify_change_password_request_with_sid,
};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_session(user_id: [u8; 16], created_at_ns: u64, channel_binding: [u8; 32]) -> Session {
    Session::new(
        user_id,
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        channel_binding,
        created_at_ns,
    )
}

#[test]
fn full_change_password_round_trip() {
    let store = SessionStore::new();
    let user_id = [1u8; 16];
    let sid = [0xaau8; 32];
    let channel_binding = [0x33u8; 32];
    let session = make_session(user_id, 100, channel_binding);
    store.insert(sid, session);
    let session_arc = store.lookup(&sid).unwrap();

    // Server-known: existing user material derived from "old password".
    let old_password = b"old correct password";
    let salt = [0x55u8; 16];
    let kdf = fast_kdf();
    let user_derived = DerivedKeys::derive(old_password, &salt, &kdf).unwrap();
    let user_stored_key = user_derived.stored_key;

    // Step 1: client builds challenge request.
    let client_nonce_cp = client_cp::build_challenge_request();

    // Step 2: server records pending state and emits challenge_cp.
    let challenge = start_change_password_challenge(
        &session_arc,
        salt,
        kdf,
        client_nonce_cp,
        UnixNanos::now().as_u64(),
    );

    // Step 3-4: client builds request from old + new password.
    let mut old_buf = old_password.to_vec();
    let mut new_buf = b"new even better password".to_vec();
    let request = client_cp::build_request(
        &NormalizedUsername::from_raw("alice").unwrap(),
        &sid,
        &client_nonce_cp,
        &challenge.server_nonce_cp,
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &channel_binding,
        &mut old_buf,
        &mut new_buf,
        kdf, // recommendation = same params for test
    )
    .unwrap();

    // Step 5: server verifies and produces apply payload.
    let apply = verify_change_password_request_with_sid(
        &session_arc,
        &sid,
        salt,
        &user_stored_key,
        kdf,
        &request,
        kdf,
        UnixNanos::now().as_u64(),
    )
    .unwrap();

    assert_eq!(apply.salt, request.new_salt);
    assert_eq!(apply.stored_key.0, request.new_stored_key);
    assert_eq!(&apply.server_key[..], &request.new_server_key[..]);

    // Step 6: server kills all sessions of this user.
    let _new_invalid_before = finalize_change_password(&store, &user_id, UnixNanos::now().as_u64());
    assert!(store.lookup(&sid).is_none());
}

#[test]
fn rejects_wrong_old_password() {
    let store = SessionStore::new();
    let user_id = [1u8; 16];
    let sid = [0xaau8; 32];
    let channel_binding = [0x33u8; 32];
    let session = make_session(user_id, 100, channel_binding);
    store.insert(sid, session);
    let session_arc = store.lookup(&sid).unwrap();

    let salt = [0x55u8; 16];
    let kdf = fast_kdf();
    let user_derived = DerivedKeys::derive(b"real old password", &salt, &kdf).unwrap();

    let client_nonce_cp = client_cp::build_challenge_request();
    let challenge = start_change_password_challenge(
        &session_arc,
        salt,
        kdf,
        client_nonce_cp,
        UnixNanos::now().as_u64(),
    );

    let mut old_buf = b"WRONG password".to_vec();
    let mut new_buf = b"new password".to_vec();
    let request = client_cp::build_request(
        &NormalizedUsername::from_raw("alice").unwrap(),
        &sid,
        &client_nonce_cp,
        &challenge.server_nonce_cp,
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &channel_binding,
        &mut old_buf,
        &mut new_buf,
        kdf,
    )
    .unwrap();

    let result = verify_change_password_request_with_sid(
        &session_arc,
        &sid,
        salt,
        &user_derived.stored_key,
        kdf,
        &request,
        kdf,
        UnixNanos::now().as_u64(),
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn rejects_when_no_pending_challenge() {
    let store = SessionStore::new();
    let user_id = [1u8; 16];
    let sid = [0xaau8; 32];
    store.insert(sid, make_session(user_id, 100, [0u8; 32]));
    let session_arc = store.lookup(&sid).unwrap();

    let salt = [0x55u8; 16];
    let kdf = fast_kdf();

    // Skip Step 2 — client tries to send request without prior challenge.
    let request = shamir_connect::server::changepw::ChangePwRequest {
        client_proof_old: [0u8; 32],
        new_salt: [0x77u8; 16],
        new_stored_key: [0u8; 32],
        new_server_key: [0u8; 32],
    };

    let result = verify_change_password_request_with_sid(
        &session_arc,
        &sid,
        salt,
        &StoredKey([0u8; 32]),
        kdf,
        &request,
        kdf,
        UnixNanos::now().as_u64(),
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn rejects_after_ttl_expiration() {
    let store = SessionStore::new();
    let user_id = [1u8; 16];
    let sid = [0xaau8; 32];
    let channel_binding = [0x33u8; 32];
    store.insert(sid, make_session(user_id, 100, channel_binding));
    let session_arc = store.lookup(&sid).unwrap();

    let salt = [0x55u8; 16];
    let kdf = fast_kdf();
    let user_derived = DerivedKeys::derive(b"correct", &salt, &kdf).unwrap();

    let client_nonce_cp = client_cp::build_challenge_request();
    // Issue challenge "long ago" (10 minutes back).
    let issued_at = UnixNanos::now().as_u64() - 10 * ns::MINUTE;
    let challenge =
        start_change_password_challenge(&session_arc, salt, kdf, client_nonce_cp, issued_at);

    let mut old_buf = b"correct".to_vec();
    let mut new_buf = b"new strong password 99".to_vec();
    let request = client_cp::build_request(
        &NormalizedUsername::from_raw("alice").unwrap(),
        &sid,
        &client_nonce_cp,
        &challenge.server_nonce_cp,
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &channel_binding,
        &mut old_buf,
        &mut new_buf,
        kdf,
    )
    .unwrap();

    let result = verify_change_password_request_with_sid(
        &session_arc,
        &sid,
        salt,
        &user_derived.stored_key,
        kdf,
        &request,
        kdf,
        UnixNanos::now().as_u64(),
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn second_challenge_invalidates_first_multi_tab() {
    // Tab A starts; Tab B starts; Tab A submits → fails because Tab B's
    // challenge replaced the pending state.
    let store = SessionStore::new();
    let user_id = [1u8; 16];
    let sid = [0xaau8; 32];
    let channel_binding = [0x33u8; 32];
    store.insert(sid, make_session(user_id, 100, channel_binding));
    let session_arc = store.lookup(&sid).unwrap();

    let salt = [0x55u8; 16];
    let kdf = fast_kdf();
    let user_derived = DerivedKeys::derive(b"correct", &salt, &kdf).unwrap();

    let nonce_a = client_cp::build_challenge_request();
    let challenge_a = start_change_password_challenge(
        &session_arc,
        salt,
        kdf,
        nonce_a,
        UnixNanos::now().as_u64(),
    );

    // Tab B starts before Tab A submits → overwrites pending state.
    let nonce_b = client_cp::build_challenge_request();
    let _challenge_b = start_change_password_challenge(
        &session_arc,
        salt,
        kdf,
        nonce_b,
        UnixNanos::now().as_u64(),
    );

    // Tab A computes proof using A's nonces.
    let mut old_buf = b"correct".to_vec();
    let mut new_buf = b"new strong password 99".to_vec();
    let request_a = client_cp::build_request(
        &NormalizedUsername::from_raw("alice").unwrap(),
        &sid,
        &nonce_a,
        &challenge_a.server_nonce_cp,
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &channel_binding,
        &mut old_buf,
        &mut new_buf,
        kdf,
    )
    .unwrap();

    // Server tries to verify A's proof against B's pending state → fail.
    let result = verify_change_password_request_with_sid(
        &session_arc,
        &sid,
        salt,
        &user_derived.stored_key,
        kdf,
        &request_a,
        kdf,
        UnixNanos::now().as_u64(),
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn proof_is_single_use_after_consume() {
    // Even with the right proof, a second submission with the same proof
    // fails because pending state has been consumed.
    let store = SessionStore::new();
    let user_id = [1u8; 16];
    let sid = [0xaau8; 32];
    let channel_binding = [0x33u8; 32];
    store.insert(sid, make_session(user_id, 100, channel_binding));
    let session_arc = store.lookup(&sid).unwrap();

    let salt = [0x55u8; 16];
    let kdf = fast_kdf();
    let user_derived = DerivedKeys::derive(b"correct", &salt, &kdf).unwrap();

    let nonce = client_cp::build_challenge_request();
    let challenge =
        start_change_password_challenge(&session_arc, salt, kdf, nonce, UnixNanos::now().as_u64());

    let mut old_buf = b"correct".to_vec();
    let mut new_buf = b"new strong password 99".to_vec();
    let request = client_cp::build_request(
        &NormalizedUsername::from_raw("alice").unwrap(),
        &sid,
        &nonce,
        &challenge.server_nonce_cp,
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &channel_binding,
        &mut old_buf,
        &mut new_buf,
        kdf,
    )
    .unwrap();

    let _ok = verify_change_password_request_with_sid(
        &session_arc,
        &sid,
        salt,
        &user_derived.stored_key,
        kdf,
        &request,
        kdf,
        UnixNanos::now().as_u64(),
    )
    .unwrap();

    // Replay attempt — pending state was cleared.
    let result = verify_change_password_request_with_sid(
        &session_arc,
        &sid,
        salt,
        &user_derived.stored_key,
        kdf,
        &request,
        kdf,
        UnixNanos::now().as_u64(),
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn finalize_kills_only_target_user_sessions() {
    let store = SessionStore::new();
    let alice_uid = [1u8; 16];
    let bob_uid = [2u8; 16];
    store.insert([0xa1u8; 32], make_session(alice_uid, 100, [0u8; 32]));
    store.insert([0xa2u8; 32], make_session(alice_uid, 100, [0u8; 32]));
    store.insert([0xb1u8; 32], make_session(bob_uid, 100, [0u8; 32]));

    let _ = finalize_change_password(&store, &alice_uid, UnixNanos::now().as_u64());
    assert_eq!(store.len(), 1); // only bob remains
    assert!(store.lookup(&[0xb1u8; 32]).is_some());
}

/// Spec §3.2 NORMATIVE / diagram 04: client-side password policy must reject
/// `new_password` shorter than `PASSWORD_MIN_LENGTH = 12 chars` BEFORE
/// running Argon2id. The server has no way to verify this.
#[test]
fn build_request_rejects_weak_new_password_per_spec_3_2() {
    let salt = [0x55u8; 16];
    let kdf = fast_kdf();
    let username = NormalizedUsername::from_raw("alice").unwrap();
    let sid = [0xa1u8; 32];
    let client_nonce_cp = [0xc1u8; 32];
    let server_nonce_cp = [0x91u8; 32];

    // OLD password is fine — verification is on `new_password`.
    let mut old_buf = b"old correct password".to_vec();
    let mut new_buf = b"short".to_vec(); // 5 chars < 12
    let result = client_cp::build_request(
        &username,
        &sid,
        &client_nonce_cp,
        &server_nonce_cp,
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &mut old_buf,
        &mut new_buf,
        kdf,
    );
    assert!(matches!(
        result,
        Err(shamir_connect::common::error::Error::InvalidPassword(_))
    ));
}

/// Spec §3.2: single-repeated-char passwords MUST be rejected client-side.
#[test]
fn build_request_rejects_single_repeated_char_new_password() {
    let salt = [0x55u8; 16];
    let kdf = fast_kdf();
    let username = NormalizedUsername::from_raw("alice").unwrap();
    let sid = [0xa1u8; 32];

    let mut old_buf = b"old correct password".to_vec();
    let mut new_buf = b"aaaaaaaaaaaaaaaa".to_vec(); // 16 'a's
    let result = client_cp::build_request(
        &username,
        &sid,
        &[0u8; 32],
        &[0u8; 32],
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &mut old_buf,
        &mut new_buf,
        kdf,
    );
    assert!(matches!(
        result,
        Err(shamir_connect::common::error::Error::InvalidPassword(_))
    ));
}
