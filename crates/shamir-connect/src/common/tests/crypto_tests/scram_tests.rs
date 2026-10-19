//! Tests for SCRAM derivation (spec §3.3, §5.1.3).
//!
//! End-to-end round-trip is the most informative — derive on "client", verify
//! on "server" using public values only. This pins the entire SCRAM
//! arithmetic.

use crate::common::auth_message::{AuthMessage, AuthMessageInputs};
use crate::common::crypto::sha256;
use crate::common::kdf_params::KdfParams;
use crate::common::scram::{
    build_client_proof, build_server_signature, recover_client_key, verify_client_proof,
    DerivedKeys,
};
use crate::common::types::{BindingMode, ProtocolVersion, TransportKind};
use crate::common::username::NormalizedUsername;

/// Fast Argon2id params for tests (real defaults take ~2s and we run many tests).
fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn build_test_auth_message(username: &str) -> AuthMessage {
    let user = NormalizedUsername::from_raw(username).unwrap();
    let mut nonce = [0u8; 32];
    nonce[0] = 0xab;
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
fn scram_full_round_trip_succeeds() {
    let password = b"correct horse battery staple";
    let salt = [0x42u8; 16];
    let params = fast_kdf();

    // Client: derive everything
    let derived = DerivedKeys::derive(password, &salt, &params).unwrap();
    let am = build_test_auth_message("alice");

    let proof = build_client_proof(&derived.client_key, &derived.stored_key, &am);

    // Server: verify using only stored_key (what's in __system__/users)
    assert!(verify_client_proof(&proof, &derived.stored_key, &am));
}

#[test]
fn scram_rejects_wrong_password() {
    let salt = [0x42u8; 16];
    let params = fast_kdf();
    let real = DerivedKeys::derive(b"correct password", &salt, &params).unwrap();
    let attacker = DerivedKeys::derive(b"wrong password", &salt, &params).unwrap();

    let am = build_test_auth_message("alice");
    let attacker_proof = build_client_proof(&attacker.client_key, &attacker.stored_key, &am);

    // Server has the REAL stored_key, attacker sent a proof for a different one.
    assert!(!verify_client_proof(&attacker_proof, &real.stored_key, &am));
}

#[test]
fn scram_rejects_proof_for_different_auth_message() {
    let password = b"correct password";
    let salt = [0x42u8; 16];
    let params = fast_kdf();
    let derived = DerivedKeys::derive(password, &salt, &params).unwrap();

    let am1 = build_test_auth_message("alice");
    let am2 = build_test_auth_message("bob"); // different username → different auth_message

    let proof_for_am1 = build_client_proof(&derived.client_key, &derived.stored_key, &am1);

    // Server tries verifying against am2 — should fail.
    assert!(!verify_client_proof(
        &proof_for_am1,
        &derived.stored_key,
        &am2
    ));
}

#[test]
fn server_signature_round_trip() {
    // Simulate: server sends server_signature; client recomputes from its own
    // server_key (same value) and compares. They must match.
    let password = b"hello world";
    let salt = [0x42u8; 16];
    let params = fast_kdf();
    let derived = DerivedKeys::derive(password, &salt, &params).unwrap();
    let am = build_test_auth_message("alice");

    let server_sig_from_server = build_server_signature(&derived.server_key, &am);
    let server_sig_recomputed_by_client = build_server_signature(&derived.server_key, &am);

    assert_eq!(server_sig_from_server, server_sig_recomputed_by_client);
}

#[test]
fn server_signature_differs_for_different_keys() {
    let salt = [0x42u8; 16];
    let params = fast_kdf();
    let alice = DerivedKeys::derive(b"alice-password", &salt, &params).unwrap();
    let bob = DerivedKeys::derive(b"bob-password", &salt, &params).unwrap();

    let am = build_test_auth_message("user");
    let sig_a = build_server_signature(&alice.server_key, &am);
    let sig_b = build_server_signature(&bob.server_key, &am);
    assert_ne!(sig_a, sig_b);
}

#[test]
fn xor_recovery_round_trip() {
    // recover_client_key(proof, sig) followed by xor with sig is identity.
    let key = [0x33u8; 32];
    let sig = [0x77u8; 32];
    let mut proof = [0u8; 32];
    for i in 0..32 {
        proof[i] = key[i] ^ sig[i];
    }
    assert_eq!(recover_client_key(&proof, &sig), key);
}

#[test]
fn stored_key_equals_sha256_of_client_key() {
    let password = b"test";
    let salt = [0u8; 16];
    let params = fast_kdf();
    let derived = DerivedKeys::derive(password, &salt, &params).unwrap();
    assert_eq!(sha256(&derived.client_key[..]), derived.stored_key.0);
}

#[test]
fn derive_deterministic_for_same_inputs() {
    let password = b"deterministic";
    let salt = [0xaau8; 16];
    let params = fast_kdf();
    let a = DerivedKeys::derive(password, &salt, &params).unwrap();
    let b = DerivedKeys::derive(password, &salt, &params).unwrap();
    assert_eq!(a.salted_password, b.salted_password);
    assert_eq!(a.client_key, b.client_key);
    assert_eq!(a.server_key, b.server_key);
    assert_eq!(a.stored_key.0, b.stored_key.0);
}

#[test]
fn derive_differs_for_different_salt() {
    let password = b"same password";
    let params = fast_kdf();
    let a = DerivedKeys::derive(password, &[0x11u8; 16], &params).unwrap();
    let b = DerivedKeys::derive(password, &[0x22u8; 16], &params).unwrap();
    assert_ne!(a.stored_key.0, b.stored_key.0);
}
