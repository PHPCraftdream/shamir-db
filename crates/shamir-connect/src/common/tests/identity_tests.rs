//! Tests for `identity_input` build + Ed25519 sign/verify (spec §5.2.4, §5.3).

use crate::common::auth_message::{AuthMessage, AuthMessageInputs};
use crate::common::crypto::Ed25519Keypair;
use crate::common::domain_tags::IDENTITY_V1;
use crate::common::identity::{build_identity_input, sign_identity, verify_identity};
use crate::common::kdf_params::KdfParams;
use crate::common::types::{BindingMode, ProtocolVersion, TransportKind};
use crate::common::username::NormalizedUsername;

fn fixture_auth_message() -> AuthMessage {
    let user = NormalizedUsername::from_raw("alice").unwrap();
    let mut nonce = [0u8; 32];
    nonce[0] = 0x42;
    let zero32 = [0u8; 32];
    let salt = [0x55u8; 16];

    AuthMessage::build(AuthMessageInputs {
        username: &user,
        client_nonce: &nonce,
        server_nonce: &nonce,
        salt: &salt,
        kdf_params: KdfParams::DEFAULT,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &zero32,
        supported_version: ProtocolVersion::V1,
    })
    .unwrap()
}

#[test]
fn identity_input_starts_with_domain_tag() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];

    let input = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        1_700_000_000_000_000_000,
    );

    assert_eq!(&input[..IDENTITY_V1.len()], IDENTITY_V1);
}

#[test]
fn identity_input_length_matches_formula() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];

    let input = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        0,
    );

    // 18 (tag) + 32 (sha256(pub)) + 1 + 1 + 32 + am.len() + 32 + 8
    assert_eq!(input.len(), 18 + 32 + 1 + 1 + 32 + am.len() + 32 + 8);
}

#[test]
fn sign_then_verify_succeeds() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];
    let expires_at_ns = 1_700_000_000_000_000_000u64;

    let input = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        expires_at_ns,
    );

    let sig = sign_identity(&kp, &input);
    assert!(verify_identity(&kp.public_bytes(), &input, &sig));
}

#[test]
fn verify_fails_with_different_session_id() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let expires_at_ns = 1u64;

    let input1 = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &[0xaau8; 32],
        expires_at_ns,
    );
    let input2 = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &[0xbbu8; 32],
        expires_at_ns,
    );
    let sig = sign_identity(&kp, &input1);
    assert!(!verify_identity(&kp.public_bytes(), &input2, &sig));
}

#[test]
fn verify_fails_with_different_transport_kind() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];

    let tcp_input = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        0,
    );
    let ws_input = build_identity_input(
        &kp.public_bytes(),
        TransportKind::WebSocket,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        0,
    );
    let sig = sign_identity(&kp, &tcp_input);
    assert!(!verify_identity(&kp.public_bytes(), &ws_input, &sig));
}

#[test]
fn verify_fails_with_different_binding_mode() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];

    let tls_input = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        0,
    );
    let none_input = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::None,
        &[0u8; 32],
        &am,
        &session_id,
        0,
    );
    let sig = sign_identity(&kp, &tls_input);
    assert!(!verify_identity(&kp.public_bytes(), &none_input, &sig));
}

#[test]
fn verify_fails_with_tampered_tls_exporter() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];

    let exporter_a = [0xaau8; 32];
    let exporter_b = [0xbbu8; 32];

    let input_a = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &exporter_a,
        &am,
        &session_id,
        0,
    );
    let input_b = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &exporter_b,
        &am,
        &session_id,
        0,
    );
    let sig = sign_identity(&kp, &input_a);
    assert!(!verify_identity(&kp.public_bytes(), &input_b, &sig));
}

#[test]
fn verify_fails_with_different_expires_at_ns() {
    let kp = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];

    let input1 = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        1_700_000_000_000_000_000,
    );
    let input2 = build_identity_input(
        &kp.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        1_800_000_000_000_000_000,
    );
    let sig = sign_identity(&kp, &input1);
    assert!(!verify_identity(&kp.public_bytes(), &input2, &sig));
}

#[test]
fn verify_fails_with_wrong_pub_key() {
    let kp1 = Ed25519Keypair::generate();
    let kp2 = Ed25519Keypair::generate();
    let am = fixture_auth_message();
    let session_id = [0u8; 32];

    let input = build_identity_input(
        &kp1.public_bytes(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &[0u8; 32],
        &am,
        &session_id,
        0,
    );

    let sig = sign_identity(&kp1, &input);
    // Even with the same input string, sig from kp1 won't verify against kp2's pubkey.
    assert!(!verify_identity(&kp2.public_bytes(), &input, &sig));
}
