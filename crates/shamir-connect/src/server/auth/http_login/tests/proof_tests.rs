use crate::common::crypto::{hmac_sha256, sha256, ClientKey, ServerKey, StoredKey};
use crate::common::domain_tags::{CLIENT_KEY, SERVER_KEY};
use crate::common::scram::ClientProof;
use crate::server::http_login::{build_server_signature, verify_http_login_proof};
use zeroize::Zeroizing;

#[test]
fn proof_uses_scram_schedule_and_rejects_invalid_controls() {
    let transcript = b"http-login transcript fixture";
    let salted: Zeroizing<[u8; 32]> = Zeroizing::new([0x31; 32]);
    let client_key: ClientKey = Zeroizing::new(hmac_sha256(&salted[..], CLIENT_KEY));
    let server_key: ServerKey = Zeroizing::new(hmac_sha256(&salted[..], SERVER_KEY));
    let stored = StoredKey(sha256(&client_key[..]));
    let signature = hmac_sha256(&stored.0, transcript);
    let proof: ClientProof = core::array::from_fn(|index| client_key[index] ^ signature[index]);
    assert!(verify_http_login_proof(&proof, &stored, transcript));
    let mut changed = proof;
    changed[0] ^= 1;
    assert!(!verify_http_login_proof(&changed, &stored, transcript));
    let altered_transcript = b"Http-login transcript fixture";
    assert!(!verify_http_login_proof(
        &proof,
        &stored,
        altered_transcript
    ));
    assert!(!verify_http_login_proof(
        &proof,
        &StoredKey([0x99; 32]),
        transcript
    ));
    assert!(!verify_http_login_proof(
        &proof,
        &stored,
        &transcript[..transcript.len() - 1]
    ));
    assert!(!verify_http_login_proof(&proof, &stored, b""));

    let server_signature = build_server_signature(&server_key, transcript);
    let client_recomputed = hmac_sha256(&server_key[..], transcript);
    assert_eq!(server_signature, client_recomputed);
    let wrong_server_key = [0x99; 32];
    assert_ne!(
        server_signature,
        build_server_signature(&wrong_server_key, transcript)
    );
    assert_ne!(
        server_signature,
        build_server_signature(&server_key, b"altered transcript")
    );
}
