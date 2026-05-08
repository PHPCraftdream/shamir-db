//! In-memory user record for server SCRAM verification.
//!
//! Real deployments persist this to `__system__/users/{user_id}` per spec §3.5.
//! Here we only model what SCRAM verify needs.

use crate::common::crypto::StoredKey;
use crate::common::kdf_params::KdfParams;
use crate::common::types::limits;
use zeroize::Zeroizing;

/// Persisted user record (SCRAM-relevant fields only).
#[derive(Clone)]
pub struct UserRecord {
    /// Per-user 16-byte Argon2id salt.
    pub salt: [u8; limits::SALT_BYTES],
    /// SHA256(client_key) — what the server stores for verification.
    pub stored_key: StoredKey,
    /// HMAC(salted_password, "Server Key") — used for `server_signature`.
    pub server_key: Zeroizing<[u8; 32]>,
    /// Argon2id parameters that produced `stored_key` / `server_key`.
    pub kdf_params: KdfParams,
    /// `tickets_invalid_before_ns` — anything ≤ this → resume rejected (spec §3.5).
    /// **INITIAL VALUE = 0** at createUser/bootstrap so first login passes.
    pub tickets_invalid_before_ns: u64,
}
