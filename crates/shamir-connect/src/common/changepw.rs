//! `changePassword` flow primitives (spec §12.5).
//!
//! Two-step flow with fresh per-session challenge. The shared `auth_message_cp`
//! must be byte-exactly reproduced by both sides; layout per spec §12.5:
//!
//! ```text
//! auth_message_cp =
//!     "SHAMIR-CHGPW-v1"
//!  || u16_be(byte_len(username_nfc)) || username_nfc
//!  || session_id(32)
//!  || client_nonce_cp(32)
//!  || server_nonce_cp(32)
//!  || salt(16)
//!  || u32_be(memory_kb) || u32_be(time) || u32_be(parallelism) || u8(argon2_version)
//!  || u8(transport_kind)
//!  || u8(binding_mode)
//!  || channel_binding_at_auth(32)
//! ```

use crate::common::domain_tags::CHGPW_V1;
use crate::common::error::{Error, Result};
use crate::common::kdf_params::KdfParams;
use crate::common::time::ns;
use crate::common::types::{limits, BindingMode, TransportKind};
use crate::common::username::NormalizedUsername;

/// `changePassword` challenge TTL (spec §12.5).
pub const CHANGEPW_CHALLENGE_TTL_NS: u64 = 5 * ns::MINUTE;

/// Inputs to [`build_auth_message_cp`].
#[derive(Debug, Clone, Copy)]
pub struct ChangePwAuthMessageInputs<'a> {
    /// Already-normalized username.
    pub username: &'a NormalizedUsername,
    /// Active session id.
    pub session_id: &'a [u8; limits::SESSION_ID_BYTES],
    /// Client-supplied per-request CSPRNG nonce.
    pub client_nonce_cp: &'a [u8; 32],
    /// Server-issued per-request CSPRNG nonce.
    pub server_nonce_cp: &'a [u8; 32],
    /// User's current Argon2id salt.
    pub salt: &'a [u8; limits::SALT_BYTES],
    /// User's current KDF parameters (used to verify proof_old).
    pub kdf_params: KdfParams,
    /// Transport tag at session creation.
    pub transport_kind: TransportKind,
    /// Binding mode at session creation.
    pub binding_mode: BindingMode,
    /// `channel_binding_at_auth` snapshotted into the active session.
    pub channel_binding_at_auth: &'a [u8; 32],
}

/// Build the canonical `auth_message_cp` byte string.
pub fn build_auth_message_cp(inputs: ChangePwAuthMessageInputs<'_>) -> Result<Vec<u8>> {
    let user = inputs.username.as_bytes();
    if user.len() > limits::USERNAME_MAX_BYTES {
        return Err(Error::InvalidUsername("> 255 bytes after NFC"));
    }
    if inputs.client_nonce_cp.iter().all(|&b| b == 0) {
        return Err(Error::InvalidInput("client_nonce_cp all-zero"));
    }
    if inputs.server_nonce_cp.iter().all(|&b| b == 0) {
        return Err(Error::InvalidInput("server_nonce_cp all-zero"));
    }

    let mut out = Vec::with_capacity(
        CHGPW_V1.len() + 2 + user.len() + 32 + 32 + 32 + 16 + 4 + 4 + 4 + 1 + 1 + 1 + 32,
    );
    out.extend_from_slice(CHGPW_V1);
    out.extend_from_slice(&(user.len() as u16).to_be_bytes());
    out.extend_from_slice(user);
    out.extend_from_slice(inputs.session_id);
    out.extend_from_slice(inputs.client_nonce_cp);
    out.extend_from_slice(inputs.server_nonce_cp);
    out.extend_from_slice(inputs.salt);
    out.extend_from_slice(&inputs.kdf_params.memory_kb.to_be_bytes());
    out.extend_from_slice(&inputs.kdf_params.time.to_be_bytes());
    out.extend_from_slice(&inputs.kdf_params.parallelism.to_be_bytes());
    out.push(inputs.kdf_params.argon2_version);
    out.push(inputs.transport_kind.as_u8());
    out.push(inputs.binding_mode.as_u8());
    out.extend_from_slice(inputs.channel_binding_at_auth);
    Ok(out)
}
