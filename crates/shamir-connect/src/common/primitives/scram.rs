//! SCRAM key derivation primitives (spec §3.3 / §5.1.3).
//!
//! Shared between client and server: same arithmetic both sides, same
//! constant-time discipline.
//!
//! ```text
//! salted_password = Argon2id(password, salt, kdf_params)
//! client_key      = HMAC-SHA256(salted_password, "Client Key")
//! server_key      = HMAC-SHA256(salted_password, "Server Key")
//! stored_key      = SHA256(client_key)
//!
//! client_signature = HMAC-SHA256(stored_key, auth_message)
//! client_proof     = client_key XOR client_signature
//! server_signature = HMAC-SHA256(server_key, auth_message)
//! ```
//!
//! The verifying side recovers `client_key` via `client_proof XOR client_signature`
//! (since HMAC output XOR is reversible) and compares
//! `SHA256(recovered_client_key) ?= stored_key` in constant time (RFC 5802 §3).

use crate::common::auth_message::AuthMessage;
use crate::common::crypto::{
    argon2id, constant_time_eq, hmac_sha256, sha256, ClientKey, HmacTag, SaltedPassword, ServerKey,
    StoredKey,
};
use crate::common::domain_tags::{CLIENT_KEY, SERVER_KEY};
use crate::common::error::Result;
use crate::common::kdf_params::KdfParams;
use zeroize::Zeroizing;

/// 32-byte SCRAM proof — the value transmitted on the wire as `client_proof`.
pub type ClientProof = [u8; 32];

/// All four derived values from a single Argon2id pass. Held by the client
/// across handshake steps; on success it can be partially zeroized as we
/// no longer need `client_key` after building the proof.
pub struct DerivedKeys {
    /// Output of Argon2id (zeroized on drop).
    pub salted_password: SaltedPassword,
    /// `client_key` = HMAC(salted_password, "Client Key") — zeroized on drop.
    pub client_key: ClientKey,
    /// `server_key` = HMAC(salted_password, "Server Key") — zeroized on drop.
    pub server_key: ServerKey,
    /// `stored_key` = SHA256(client_key) — public, retained.
    pub stored_key: StoredKey,
}

impl DerivedKeys {
    /// Run the full client-side derivation: Argon2id then HMACs and SHA-256.
    /// Caller is expected to zeroize the input password slice afterwards.
    pub fn derive(password: &[u8], salt: &[u8], params: &KdfParams) -> Result<Self> {
        let salted_password = argon2id(password, salt, params)?;
        let client_key_tag = hmac_sha256(&salted_password[..], CLIENT_KEY);
        let server_key_tag = hmac_sha256(&salted_password[..], SERVER_KEY);

        let client_key: ClientKey = Zeroizing::new(client_key_tag);
        let server_key: ServerKey = Zeroizing::new(server_key_tag);
        let stored = StoredKey(sha256(&client_key[..]));

        Ok(Self {
            salted_password,
            client_key,
            server_key,
            stored_key: stored,
        })
    }
}

/// Compute `client_proof = client_key XOR HMAC(stored_key, auth_message)`.
pub fn build_client_proof(
    client_key: &ClientKey,
    stored_key: &StoredKey,
    auth_message: &AuthMessage,
) -> ClientProof {
    let signature = hmac_sha256(&stored_key.0, auth_message.as_bytes());
    xor_32(client_key, &signature)
}

/// Recover `client_key` from a transmitted `client_proof` and locally
/// recomputed `client_signature = HMAC(stored_key, auth_message)`.
///
/// Used by the server to recover what the client sent, then check
/// `SHA256(recovered) ?= stored_key` (see [`verify_client_proof`]).
pub fn recover_client_key(client_proof: &ClientProof, client_signature: &HmacTag) -> [u8; 32] {
    xor_32(client_proof, client_signature)
}

/// Server-side SCRAM check: returns true iff the client's proof is consistent
/// with the persisted `stored_key`. Performed in constant time (`subtle::ct_eq`).
///
/// `stored_key_or_fake` is either the real persisted key for the user OR the
/// fake value from [`crate::common::fake_blob::FakeBlob`] — the surrounding
/// branch must be constant-time so this function alone can't distinguish.
pub fn verify_client_proof(
    client_proof: &ClientProof,
    stored_key_or_fake: &StoredKey,
    auth_message: &AuthMessage,
) -> bool {
    let signature = hmac_sha256(&stored_key_or_fake.0, auth_message.as_bytes());
    let recovered = recover_client_key(client_proof, &signature);
    let recomputed = sha256(&recovered);
    constant_time_eq(&recomputed, &stored_key_or_fake.0)
}

/// Compute `server_signature = HMAC(server_key, auth_message)`.
///
/// Server sends this in `auth_ok` for client-side mutual authentication
/// (RFC 5802 §3). Client recomputes from its own `server_key` and compares.
pub fn build_server_signature(
    server_key_or_fake: &[u8; 32],
    auth_message: &AuthMessage,
) -> HmacTag {
    hmac_sha256(server_key_or_fake, auth_message.as_bytes())
}

/// XOR two 32-byte arrays into a fresh array. (Bit-wise, byte by byte.)
fn xor_32(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = a[i] ^ b[i];
    }
    out
}
