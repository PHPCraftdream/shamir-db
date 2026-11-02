use crate::common::crypto::{constant_time_eq, hmac_sha256, sha256, StoredKey};
use crate::common::scram::ClientProof;
use zeroize::Zeroizing;

/// Verify a HTTP-login SCRAM proof against the stored verifier and transcript.
pub fn verify_http_login_proof(proof: &ClientProof, stored_key: &StoredKey, transcript: &[u8]) -> bool {
    let signature = hmac_sha256(&stored_key.0, transcript);
    let recovered: Zeroizing<[u8; 32]> = Zeroizing::new(core::array::from_fn(|i| proof[i] ^ signature[i]));
    let candidate = sha256(&recovered[..]);
    constant_time_eq(&candidate, &stored_key.0)
}

/// Compute `HMAC-SHA256(server_key, transcript)` for the HTTP-login response.
pub fn build_server_signature(server_key: &[u8; 32], transcript: &[u8]) -> [u8; 32] {
    hmac_sha256(server_key, transcript)
}
