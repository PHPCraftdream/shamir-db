//! Regression coverage for client-side rotation recovery and its server payload helper.

use shamir_connect::client::rotation::verify_rotation_in_progress;
use shamir_connect::client::{HandshakeBuilder, RotationRecovery, ServerAuthOk};
use shamir_connect::common::auth_message::{AuthMessage, AuthMessageInputs};
use shamir_connect::common::crypto::{sha256, Ed25519Keypair};
use shamir_connect::common::error::Error;
use shamir_connect::common::identity::build_identity_input;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::{build_client_proof, DerivedKeys};
use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::common::types::{BindingMode, ProtocolVersion, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::{
    AuthInitView, ListenerPolicy, ProofOutcome, ServerHandshake, ServerSecrets, UserRecord,
};

const NOW: u64 = 1_000_000_000_000;

fn params() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn client_and_message() -> (
    shamir_connect::client::ClientHandshake,
    DerivedKeys,
    AuthMessage,
) {
    let username = NormalizedUsername::from_raw("alice").expect("username");
    let client = HandshakeBuilder::new(username, TransportKind::Tcp, BindingMode::TlsExporter)
        .pinned_hash(sha256(&Ed25519Keypair::from_seed(&[1; 32]).public_bytes()))
        .tls_exporter([4; 32])
        .build()
        .expect("client");
    let message = AuthMessage::build(AuthMessageInputs {
        username: client.username(),
        client_nonce: &[6; 32],
        server_nonce: &[7; 32],
        salt: &[8; 16],
        kdf_params: params(),
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &[4; 32],
        supported_version: ProtocolVersion::V1,
    })
    .expect("auth message");
    let keys = DerivedKeys::derive(b"password", &[8; 16], &params()).expect("keys");
    (client, keys, message)
}

fn recovery_payload(
    old: &Ed25519Keypair,
    current: &Ed25519Keypair,
    message: &AuthMessage,
    session_id: &[u8; 32],
    expires_at_ns: u64,
    until: u64,
) -> (ServerAuthOk, DerivedKeys) {
    let keys = DerivedKeys::derive(b"password", &[8; 16], &params()).expect("keys");
    let identity_input = build_identity_input(
        &current.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[4; 32],
        message,
        session_id,
        expires_at_ns,
    );
    let rotation_proof_input = shamir_connect::common::rotation::build_rotation_proof_input(
        &old.public_bytes(),
        &current.public_bytes(),
        until,
    );
    let ok = ServerAuthOk {
        server_signature: shamir_connect::common::scram::build_server_signature(
            &keys.server_key,
            message,
        ),
        server_pub_key: current.public_bytes(),
        identity_sig: current.sign(&identity_input),
        session_id: *session_id,
        expires_at_ns,
        resumption_ticket: None,
        resumption_expires_at_ns: None,
        rotation_in_progress: Some(shamir_connect::common::wire::RotationInProgressPayload {
            previous_pub: old.public_bytes(),
            identity_sig_previous: old.sign(&identity_input),
            transition_until_ns: until,
            rotation_proof: old.sign(&rotation_proof_input),
        }),
        kdf_upgrade_required: None,
    };
    (ok, keys)
}

#[test]
fn client_verifier_checks_all_specified_branches_and_returns_pins_without_mutation() {
    let (client, keys, message) = client_and_message();
    let old = Ed25519Keypair::from_seed(&[1; 32]);
    let current = Ed25519Keypair::from_seed(&[2; 32]);
    let (ok, _) = recovery_payload(
        &old,
        &current,
        &message,
        &[5; 32],
        NOW + ns::HOUR,
        NOW + ns::DAY,
    );
    let recovered = client
        .verify_rotation_recovery(&ok, &keys, &message, NOW)
        .expect("valid recovery");
    assert_eq!(
        recovered,
        RotationRecovery {
            previous_pin: sha256(&old.public_bytes()),
            new_pin: sha256(&current.public_bytes()),
            transition_until_ns: NOW + ns::DAY,
        }
    );

    let mut bad = ok.clone();
    bad.server_signature[0] ^= 1;
    assert!(matches!(
        client.verify_rotation_recovery(&bad, &keys, &message, NOW),
        Err(Error::ServerAuthFailed)
    ));
    let no_pin = HandshakeBuilder::new(
        NormalizedUsername::from_raw("alice").expect("username"),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
    )
    .accept_new_host(true)
    .build()
    .expect("unpinned client");
    assert!(matches!(
        no_pin.verify_rotation_recovery(&ok, &keys, &message, NOW),
        Err(Error::ServerIdentityChanged)
    ));
    let same_pin = HandshakeBuilder::new(
        NormalizedUsername::from_raw("alice").expect("username"),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
    )
    .pinned_hash(sha256(&current.public_bytes()))
    .tls_exporter([4; 32])
    .build()
    .expect("client");
    assert!(matches!(
        same_pin.verify_rotation_recovery(&ok, &keys, &message, NOW),
        Err(Error::ServerIdentityChanged)
    ));
    let mut missing = ok.clone();
    missing.rotation_in_progress = None;
    assert!(matches!(
        client.verify_rotation_recovery(&missing, &keys, &message, NOW),
        Err(Error::ServerIdentityChanged)
    ));
    let mut bad_identity = ok.clone();
    bad_identity.identity_sig[0] ^= 1;
    assert!(matches!(
        client.verify_rotation_recovery(&bad_identity, &keys, &message, NOW),
        Err(Error::ServerSignatureInvalid)
    ));
    let mut bad_proof = ok.clone();
    bad_proof
        .rotation_in_progress
        .as_mut()
        .expect("payload")
        .rotation_proof[0] ^= 1;
    assert!(matches!(
        client.verify_rotation_recovery(&bad_proof, &keys, &message, NOW),
        Err(Error::ServerSignatureInvalid)
    ));
    let mut bad_previous_sig = ok.clone();
    bad_previous_sig
        .rotation_in_progress
        .as_mut()
        .expect("payload")
        .identity_sig_previous[0] ^= 1;
    assert!(matches!(
        client.verify_rotation_recovery(&bad_previous_sig, &keys, &message, NOW),
        Err(Error::ServerSignatureInvalid)
    ));
    for until in [NOW, NOW - 1, NOW + 7 * ns::DAY + ns::HOUR + 1] {
        let (bad_window, _) =
            recovery_payload(&old, &current, &message, &[5; 32], NOW + ns::HOUR, until);
        assert!(matches!(
            client.verify_rotation_recovery(&bad_window, &keys, &message, NOW),
            Err(Error::ServerSignatureInvalid)
        ));
    }
    let max_until = NOW.saturating_add(7 * ns::DAY + ns::HOUR);
    let (max_window, _) = recovery_payload(
        &old,
        &current,
        &message,
        &[5; 32],
        NOW + ns::HOUR,
        max_until,
    );
    assert!(client
        .verify_rotation_recovery(&max_window, &keys, &message, NOW)
        .is_ok());
    let wrong_pin = HandshakeBuilder::new(
        NormalizedUsername::from_raw("alice").expect("username"),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
    )
    .pinned_hash([99; 32])
    .tls_exporter([4; 32])
    .build()
    .expect("client");
    assert!(matches!(
        wrong_pin.verify_rotation_recovery(&ok, &keys, &message, NOW),
        Err(Error::ServerIdentityChanged)
    ));
}

#[test]
fn rotation_verifier_requires_explicit_approval_and_checks_previous_signatures() {
    let (client, keys, message) = client_and_message();
    let _ = (client, keys);
    let old = Ed25519Keypair::from_seed(&[1; 32]);
    let current = Ed25519Keypair::from_seed(&[2; 32]);
    let (ok, _) = recovery_payload(
        &old,
        &current,
        &message,
        &[5; 32],
        NOW + ns::HOUR,
        NOW + ns::DAY,
    );
    let payload = ok.rotation_in_progress.as_ref().expect("payload");
    let input = build_identity_input(
        &ok.server_pub_key,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[4; 32],
        &message,
        &ok.session_id,
        ok.expires_at_ns,
    );
    let pin = sha256(&old.public_bytes());
    assert!(
        verify_rotation_in_progress(payload, &ok.server_pub_key, &input, &pin, NOW, false).is_err()
    );
    assert_eq!(
        verify_rotation_in_progress(payload, &ok.server_pub_key, &input, &pin, NOW, true)
            .expect("verified"),
        sha256(&current.public_bytes())
    );
}

#[test]
fn server_helper_returns_none_outside_window_and_on_key_mismatch_and_signed_payload_on_match() {
    let secrets = ServerSecrets {
        server_secret: [10; 32],
        lockout_secret: [11; 32],
    };
    let username = NormalizedUsername::from_raw("alice").expect("username");
    let auth_init = AuthInitView {
        user: username.clone(),
        client_nonce: [12; 32],
        binding_mode: BindingMode::TlsExporter,
        version: 1,
    };
    let keys = DerivedKeys::derive(b"password", &[8; 16], &params()).expect("keys");
    let user_record = UserRecord {
        salt: [8; 16],
        stored_key: keys.stored_key,
        server_key: keys.server_key.clone(),
        kdf_params: params(),
        tickets_invalid_before_ns: 0,
    };
    let handshake = ServerHandshake::new(
        ListenerPolicy::new(BindingMode::TlsExporter),
        TransportKind::Tcp,
        &secrets,
        auth_init,
        [4; 32],
        params(),
        |_| Some(user_record),
    )
    .expect("handshake");
    let challenge = handshake.challenge();
    let message = AuthMessage::build(AuthMessageInputs {
        username: &username,
        client_nonce: &[12; 32],
        server_nonce: &challenge.server_nonce,
        salt: &challenge.salt,
        kdf_params: challenge.kdf_params,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &[4; 32],
        supported_version: ProtocolVersion::V1,
    })
    .expect("message");
    let proof = build_client_proof(&keys.client_key, &keys.stored_key, &message);
    let current = Ed25519Keypair::from_seed(&[2; 32]);
    let old = Ed25519Keypair::from_seed(&[1; 32]);
    let now = UnixNanos::now().as_u64();
    let outside = ServerIdentityState::from_material(&[2; 32], Some(&[1; 32]), Some(now), 1);
    let identity_state = ServerIdentityState::from_material(
        &[2; 32],
        Some(&[1; 32]),
        Some(now.saturating_add(ns::DAY)),
        1,
    );
    let view = match handshake
        .verify_proof(&proof, &current, ns::DAY)
        .expect("proof")
    {
        ProofOutcome::Accepted(view) => *view,
        ProofOutcome::Rejected => panic!("valid test proof rejected"),
    };
    assert!(handshake
        .rotation_in_progress_payload(&view, &outside)
        .expect("outside")
        .is_none());
    let mismatched = ServerIdentityState::from_material(
        &[3; 32],
        Some(&[1; 32]),
        Some(now.saturating_add(ns::DAY)),
        1,
    );
    assert!(handshake
        .rotation_in_progress_payload(&view, &mismatched)
        .expect("mismatch")
        .is_none());
    let payload = handshake
        .rotation_in_progress_payload(&view, &identity_state)
        .expect("active")
        .expect("payload");
    let input = build_identity_input(
        &view.server_pub_key,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[4; 32],
        &message,
        &view.session_id,
        view.expires_at_ns,
    );
    assert_eq!(
        verify_rotation_in_progress(
            &payload,
            &view.server_pub_key,
            &input,
            &sha256(&old.public_bytes()),
            now,
            true
        )
        .expect("valid server payload"),
        sha256(&view.server_pub_key)
    );
}
