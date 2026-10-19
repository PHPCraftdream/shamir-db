//! Anti-enumeration `fake_blob` (spec §5.2.1).
//!
//! For an unknown user the server **MUST** synthesize plausible
//! `(salt, stored_key, server_key)` from a deterministic HKDF over
//! `server_secret` so that the timing and structure of the failure path
//! match the real-user path bit-for-bit.
//!
//! ```text
//! fake_blob = HKDF-SHA256(
//!     ikm  = server_secret,
//!     salt = "SHAMIR-FAKE-SALT-v1",
//!     info = username_nfc,
//!     L    = 80
//! )
//! fake_salt        = fake_blob[0..16]
//! fake_stored_key  = fake_blob[16..48]
//! fake_server_key  = fake_blob[48..80]
//! ```
//!
//! `server_secret` rotation: spec §5.2.1 [NORMATIVE] requires that during a
//! rotation overlap window the **current** `server_secret` is used (NOT
//! `previous`) — otherwise an attacker observing timing across the boundary
//! could detect the rotation event. `previous` is kept only for rare backward-
//! compat decryption needs (none in v1).

use crate::common::crypto::{hkdf_sha256, StoredKey};
use crate::common::domain_tags::FAKE_SALT_V1;
use crate::common::error::Result;
use crate::common::types::limits;
use crate::common::username::NormalizedUsername;
use zeroize::Zeroizing;

/// Deterministic per-user fake material for the unknown-user path.
///
/// All three fields have the same byte size and use as their real
/// counterparts so SCRAM verify can run in branch-equivalent code.
pub struct FakeBlob {
    /// 16 bytes — substitutes a real Argon2id salt.
    pub salt: [u8; limits::SALT_BYTES],
    /// 32 bytes — substitutes the persisted `stored_key`.
    pub stored_key: StoredKey,
    /// 32 bytes — substitutes the persisted `server_key`.
    /// Wrapped in [`Zeroizing`] like a real `server_key` would be.
    pub server_key: Zeroizing<[u8; 32]>,
}

impl FakeBlob {
    /// Derive the fake triple deterministically from `(server_secret, username)`.
    ///
    /// `server_secret` MUST be the **current** secret during a rotation window
    /// (spec §5.2.1).
    pub fn derive(server_secret: &[u8; 32], username: &NormalizedUsername) -> Result<Self> {
        let mut blob = [0u8; 80];
        hkdf_sha256(server_secret, FAKE_SALT_V1, username.as_bytes(), &mut blob)?;

        let mut salt = [0u8; limits::SALT_BYTES];
        salt.copy_from_slice(&blob[0..16]);

        let mut stored = [0u8; 32];
        stored.copy_from_slice(&blob[16..48]);

        let mut server = Zeroizing::new([0u8; 32]);
        server.copy_from_slice(&blob[48..80]);

        // Wipe the temp buffer after splitting (zeroize crate would normally
        // give us this on drop, but it's a stack array — copy-then-overwrite).
        let mut zeroize_me = blob;
        zeroize_me.fill(0);

        Ok(Self {
            salt,
            stored_key: StoredKey(stored),
            server_key: server,
        })
    }
}
