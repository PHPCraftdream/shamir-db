//! SCRAM change-password secret material for
//! [`DbRequest::ChangePasswordVerify`](super::DbRequest::ChangePasswordVerify).
//!
//! Lives in its own module so the manual `Debug` (which redacts every byte
//! field) can sit right next to the fields it protects, while `DbRequest`
//! itself keeps a plain `#[derive(Debug)]`. As a newtype variant payload
//! under serde's internally-tagged enum representation, the struct's
//! fields serialize inline next to the `"op"` tag — wire-identical to the
//! previous struct variant.

use serde::{Deserialize, Serialize};

/// The four secret byte fields of a change-password verify request.
/// `Debug` is manual and prints `[REDACTED]` — these are SCRAM proof and
/// derived-key bytes and must never reach logs.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChangePasswordSecrets {
    /// SCRAM proof recovered from the OLD password, bound to the pending
    /// challenge issued by [`DbRequest::ChangePasswordChallenge`](super::DbRequest::ChangePasswordChallenge).
    #[serde(with = "serde_bytes")]
    pub client_proof_old: Vec<u8>,
    /// New per-user salt (CSPRNG, client-generated).
    #[serde(with = "serde_bytes")]
    pub new_salt: Vec<u8>,
    /// New `stored_key = SHA256(HMAC(new_salted_pw, "Client Key"))`.
    #[serde(with = "serde_bytes")]
    pub new_stored_key: Vec<u8>,
    /// New `server_key = HMAC(new_salted_pw, "Server Key")`.
    #[serde(with = "serde_bytes")]
    pub new_server_key: Vec<u8>,
}

impl std::fmt::Debug for ChangePasswordSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangePasswordSecrets")
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}
