//! Regression tests for RV-CONNECT-0FC58D93: the TOFU pin callback must
//! fire only AFTER Ed25519 identity verification succeeds — a handshake
//! with a valid SCRAM proof but invalid identity signature must capture
//! zero pins.

use shamir_connect::client::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::crypto::{sha256, Ed25519Keypair};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::{
    AuthInitView, ListenerPolicy, ProofOutcome, ServerHandshake, ServerSecrets, UserRecord,
    SESSION_MAX_AGE_NS,
};
use shamir_connect::Error;

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn fixed_secrets() -> ServerSecrets {
    ServerSecrets {
        server_secret: [0x42u8; 32],
        lockout_secret: [0xa5u8; 32],
    }
}

fn make_user_record(password: &[u8], salt: [u8; 16], params: KdfParams) -> UserRecord {
    let derived = DerivedKeys::derive(password, &salt, &params).unwrap();
    UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: derived.server_key,
        kdf_params: params,
        tickets_invalid_before_ns: 0,
    }
}

/// Drive a full client+server round trip with a TOFU client (no pin yet)
/// and return the client-facing `auth_ok` plus derived material.
fn round_trip_tofu(
    identity_kp: &Ed25519Keypair,
    password: &[u8],
) -> (
    shamir_connect::client::ClientHandshake,
    ServerAuthOk,
    DerivedKeys,
    shamir_connect::common::auth_message::AuthMessage,
) {
    let secrets = fixed_secrets();
    let policy = ListenerPolicy::new(BindingMode::TlsExporter);
    let salt = [0x55u8; 16];
    let params = fast_kdf();
    let server_user_db = make_user_record(password, salt, params);

    let exporter = [0x77u8; 32];
    let user_norm = NormalizedUsername::from_raw("alice").unwrap();
    let client = HandshakeBuilder::new(user_norm, TransportKind::Tcp, BindingMode::TlsExporter)
        .tls_exporter(exporter)
        .accept_new_host(true)
        .build()
        .unwrap();

    let auth_init = client.auth_init();
    let server_view = AuthInitView {
        user: NormalizedUsername::from_raw(&auth_init.user).unwrap(),
        client_nonce: auth_init.client_nonce,
        binding_mode: BindingMode::from_u8(auth_init.binding_mode).unwrap(),
        version: auth_init.version,
    };
    let server = ServerHandshake::new(
        policy,
        TransportKind::Tcp,
        &secrets,
        server_view,
        exporter,
        params,
        |_| Some(server_user_db.clone()),
    )
    .unwrap();

    let cc = server.challenge();
    let client_challenge = ServerChallenge {
        salt: cc.salt,
        kdf_params: cc.kdf_params,
        server_nonce: cc.server_nonce,
    };

    let mut buf = password.to_vec();
    let (proof, derived, am) = client
        .process_challenge(&client_challenge, &mut buf)
        .unwrap();

    let outcome = server
        .verify_proof(&proof, identity_kp, SESSION_MAX_AGE_NS)
        .unwrap();
    let ok = match outcome {
        ProofOutcome::Accepted(ok) => *ok,
        ProofOutcome::Rejected => panic!("server rejected legitimate proof"),
    };

    let auth_ok = ServerAuthOk {
        server_signature: ok.server_signature,
        server_pub_key: ok.server_pub_key,
        identity_sig: ok.identity_sig,
        session_id: ok.session_id,
        expires_at_ns: ok.expires_at_ns,
        resumption_ticket: None,
        resumption_expires_at_ns: None,
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    (client, auth_ok, derived, am)
}

#[test]
fn rv_handshake_0fc58d93_invalid_identity_sig_captures_zero_pins() {
    let identity_kp = Ed25519Keypair::generate();
    let (client, mut auth_ok, derived, am) =
        round_trip_tofu(&identity_kp, b"correct horse battery staple");

    // Valid SCRAM proof, but corrupt the Ed25519 identity signature.
    auth_ok.identity_sig[0] ^= 0xff;

    let mut pins: Vec<[u8; 32]> = Vec::new();
    let result = client.process_auth_ok(&auth_ok, &derived, &am, |pin| {
        pins.push(*pin);
    });

    assert!(matches!(result, Err(Error::ServerSignatureInvalid)));
    assert!(
        pins.is_empty(),
        "TOFU pin callback fired for a handshake rejected by identity verification"
    );
}

#[test]
fn rv_handshake_0fc58d93_success_path_still_offers_pin_once() {
    let identity_kp = Ed25519Keypair::generate();
    let (client, auth_ok, derived, am) =
        round_trip_tofu(&identity_kp, b"correct horse battery staple");

    let expected_pin = sha256(&identity_kp.public_bytes());
    let mut pins: Vec<[u8; 32]> = Vec::new();
    client
        .process_auth_ok(&auth_ok, &derived, &am, |pin| {
            pins.push(*pin);
        })
        .unwrap();

    assert_eq!(pins, vec![expected_pin]);
}
