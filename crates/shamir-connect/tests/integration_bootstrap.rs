//! Integration tests for the bootstrap flow (spec §11).

use shamir_connect::client::bootstrap as client_bs;
use shamir_connect::common::crypto::{sha256, Ed25519Keypair};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::common::types::TransportKind;
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::bootstrap::{make_bootstrap_challenge, BootstrapState};

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

#[test]
fn full_bootstrap_round_trip() {
    let state = BootstrapState::empty();
    assert!(state.is_bootstrap_allowed());

    // Server issues a token, operator delivers it out-of-band.
    let token = state
        .issue_token(ns::HOUR, UnixNanos::now().as_u64())
        .unwrap();

    // Server prepares its identity keypair.
    let kp = Ed25519Keypair::generate();
    let pin = sha256(&kp.public_bytes());
    let exporter = [0x99u8; 32];

    // Client builds hello, server responds with challenge.
    let hello = client_bs::build_hello();
    let challenge = make_bootstrap_challenge(&kp, TransportKind::Tcp, &exporter, &hello);

    // Client verifies challenge against the pin.
    client_bs::verify_challenge(
        &pin,
        TransportKind::Tcp,
        &exporter,
        &hello,
        &challenge,
        UnixNanos::now().as_u64(),
    )
    .unwrap();

    // Client derives material and sends bootstrap request.
    let mut password = b"correct horse battery staple".to_vec();
    let request = client_bs::build_request(
        *token,
        NormalizedUsername::from_raw("admin").unwrap(),
        &mut password,
        fast_kdf(),
    )
    .unwrap();

    // Server consumes the token and creates the user.
    let mut server_key_buf = zeroize::Zeroizing::new([0u8; 32]);
    server_key_buf.copy_from_slice(&request.server_key);
    let user_record = state
        .consume(
            &request.token,
            request.salt,
            shamir_connect::common::crypto::StoredKey(request.stored_key),
            server_key_buf,
            request.kdf_params,
            &fast_kdf(),
            UnixNanos::now().as_u64(),
        )
        .unwrap();

    assert_eq!(user_record.salt, request.salt);
    assert_eq!(user_record.stored_key.0, request.stored_key);
    assert_eq!(user_record.tickets_invalid_before_ns, 0);

    // Invariant: bootstrap is now permanently disabled.
    assert!(state.superuser_ever_existed());
    assert!(!state.is_bootstrap_allowed());
}

#[test]
fn pin_mismatch_aborts_before_password_leaks() {
    let state = BootstrapState::empty();
    let _ = state.issue_token(ns::HOUR, UnixNanos::now().as_u64()).unwrap();

    let kp = Ed25519Keypair::generate();
    let exporter = [0x99u8; 32];

    // Client has a wrong pin.
    let wrong_pin = [0xffu8; 32];

    let hello = client_bs::build_hello();
    let challenge = make_bootstrap_challenge(&kp, TransportKind::Tcp, &exporter, &hello);

    let result = client_bs::verify_challenge(
        &wrong_pin,
        TransportKind::Tcp,
        &exporter,
        &hello,
        &challenge,
        UnixNanos::now().as_u64(),
    );
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}

#[test]
fn rejects_token_replay_to_different_client_via_client_nonce_check() {
    let state = BootstrapState::empty();
    let _ = state.issue_token(ns::HOUR, UnixNanos::now().as_u64()).unwrap();

    let kp = Ed25519Keypair::generate();
    let pin = sha256(&kp.public_bytes());
    let exporter = [0x99u8; 32];

    // Client A sends hello, server responds.
    let hello_a = client_bs::build_hello();
    let challenge_a = make_bootstrap_challenge(&kp, TransportKind::Tcp, &exporter, &hello_a);

    // Client B has a DIFFERENT hello — receives challenge_a (replay attack).
    let hello_b = client_bs::build_hello();

    let result = client_bs::verify_challenge(
        &pin,
        TransportKind::Tcp,
        &exporter,
        &hello_b, // different nonce!
        &challenge_a,
        UnixNanos::now().as_u64(),
    );
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}

#[test]
fn rejects_clock_skew_beyond_60s() {
    let state = BootstrapState::empty();
    let _ = state.issue_token(ns::HOUR, UnixNanos::now().as_u64()).unwrap();

    let kp = Ed25519Keypair::generate();
    let pin = sha256(&kp.public_bytes());
    let exporter = [0x99u8; 32];

    let hello = client_bs::build_hello();
    let challenge = make_bootstrap_challenge(&kp, TransportKind::Tcp, &exporter, &hello);

    // Pretend we're 5 minutes in the future from server.
    let now_ns = challenge.server_time_ns + 5 * ns::MINUTE;

    let result = client_bs::verify_challenge(
        &pin,
        TransportKind::Tcp,
        &exporter,
        &hello,
        &challenge,
        now_ns,
    );
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}

#[test]
fn cannot_bootstrap_twice() {
    let state = BootstrapState::empty();
    let token = state
        .issue_token(ns::HOUR, UnixNanos::now().as_u64())
        .unwrap();

    // First consume succeeds.
    let mut sk = zeroize::Zeroizing::new([0xaau8; 32]);
    let _ = state
        .consume(
            &token,
            [0x55u8; 16],
            shamir_connect::common::crypto::StoredKey([0xbbu8; 32]),
            sk.clone(),
            fast_kdf(),
            &fast_kdf(),
            UnixNanos::now().as_u64(),
        )
        .unwrap();

    // After successful bootstrap: cannot issue a second token (invariant).
    sk.fill(0);
    let result = state.issue_token(ns::HOUR, UnixNanos::now().as_u64());
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}

#[test]
fn rejects_expired_token() {
    let state = BootstrapState::empty();
    let now = UnixNanos::now().as_u64();
    let token = state.issue_token(1, now).unwrap(); // 1 ns TTL

    let later = now + ns::SECOND;

    let result = state.consume(
        &token,
        [0x55u8; 16],
        shamir_connect::common::crypto::StoredKey([0xbbu8; 32]),
        zeroize::Zeroizing::new([0xaau8; 32]),
        fast_kdf(),
        &fast_kdf(),
        later,
    );
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));

    // After failed expired-consume, state is cleaned: bootstrap path remains
    // closed because we're under "issued but never succeeded" semantics.
    // It is OK to issue again now (token cleared by the consume's auto-cleanup).
    let _ = state.issue_token(ns::HOUR, later).unwrap();
}

#[test]
fn rejects_wrong_token() {
    let state = BootstrapState::empty();
    let now = UnixNanos::now().as_u64();
    let _real = state.issue_token(ns::HOUR, now).unwrap();

    let fake_token = [0x42u8; 32];
    let result = state.consume(
        &fake_token,
        [0x55u8; 16],
        shamir_connect::common::crypto::StoredKey([0xbbu8; 32]),
        zeroize::Zeroizing::new([0xaau8; 32]),
        fast_kdf(),
        &fast_kdf(),
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}

#[test]
fn rejects_kdf_params_below_floor() {
    let state = BootstrapState::empty();
    let now = UnixNanos::now().as_u64();
    let token = state.issue_token(ns::HOUR, now).unwrap();

    let too_weak = KdfParams {
        memory_kb: 1024, // below floor
        time: 1,
        parallelism: 1,
        argon2_version: 0x13,
    };

    let result = state.consume(
        &token,
        [0x55u8; 16],
        shamir_connect::common::crypto::StoredKey([0xbbu8; 32]),
        zeroize::Zeroizing::new([0xaau8; 32]),
        too_weak,
        &too_weak,
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}

#[test]
fn rejects_mismatched_kdf_params_vs_current() {
    let state = BootstrapState::empty();
    let now = UnixNanos::now().as_u64();
    let token = state.issue_token(ns::HOUR, now).unwrap();

    let mut other = fast_kdf();
    other.time += 1;

    let result = state.consume(
        &token,
        [0x55u8; 16],
        shamir_connect::common::crypto::StoredKey([0xbbu8; 32]),
        zeroize::Zeroizing::new([0xaau8; 32]),
        other,
        &fast_kdf(), // current
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}

#[test]
fn rehydrate_from_meta_blocks_when_already_used() {
    let state = BootstrapState::from_meta(None, None, true);
    assert!(!state.is_bootstrap_allowed());
    let result = state.issue_token(ns::HOUR, UnixNanos::now().as_u64());
    assert!(matches!(result, Err(shamir_connect::Error::BootstrapFailed)));
}
