//! Canonical `auth_message` builder (spec §4.1).
//!
//! `auth_message` is a byte string that **must** be reproducible bit-exactly
//! between client and server (and between Rust and JS implementations) — any
//! mismatch causes SCRAM proof verification to fail. Test vectors live in
//! `crates/shamir-connect/test-vectors/auth_v1/`.
//!
//! Layout (149 bytes for default kdf params and 5-byte username):
//!
//! ```text
//! "SHAMIR-AUTH-v1"                              (14 bytes ASCII fixed header)
//! u16_be(byte_len(username_nfc)) || username_nfc
//! client_nonce(32)
//! server_nonce(32)
//! salt(16)
//! u32_be(memory_kb)
//! u32_be(time)
//! u32_be(parallelism)
//! u8(argon2_version)         // 0x13
//! u8(transport_kind)
//! u8(binding_mode)
//! tls_exporter_or_zeros(32)
//! u8(supported_version)      // = 1
//! ```

use crate::common::domain_tags::AUTH_V1;
use crate::common::error::{Error, Result};
use crate::common::kdf_params::KdfParams;
use crate::common::types::{limits, BindingMode, ProtocolVersion, TransportKind};
use crate::common::username::NormalizedUsername;

/// Canonical `auth_message` byte string.
///
/// Construct via [`AuthMessage::build`]; the resulting bytes are passed to
/// HMAC-SHA256 for SCRAM proof / server signature, and to Ed25519 sign for
/// `identity_sig` (where they appear inside `identity_input`, spec §5.2.4).
#[derive(Debug, Clone)]
pub struct AuthMessage {
    bytes: Vec<u8>,
}

/// Inputs for [`AuthMessage::build`].
#[derive(Debug, Clone, Copy)]
pub struct AuthMessageInputs<'a> {
    /// Already-normalized username (post PRECIS+NFC).
    pub username: &'a NormalizedUsername,
    /// 32-byte CSPRNG client nonce.
    pub client_nonce: &'a [u8; limits::CLIENT_NONCE_BYTES],
    /// 32-byte CSPRNG server nonce.
    pub server_nonce: &'a [u8; limits::SERVER_NONCE_BYTES],
    /// 16-byte Argon2id salt (per-user).
    pub salt: &'a [u8; limits::SALT_BYTES],
    /// KDF parameters (raw, included as bytes — not hashed).
    pub kdf_params: KdfParams,
    /// Transport tag (tcp / ws).
    pub transport_kind: TransportKind,
    /// Binding mode (none / tls_exporter / tls_no_export).
    pub binding_mode: BindingMode,
    /// 32-byte TLS exporter (or zeros for binding_mode != TlsExporter).
    pub tls_exporter_or_zeros: &'a [u8; 32],
    /// Supported protocol version (currently always v1).
    pub supported_version: ProtocolVersion,
}

impl AuthMessage {
    /// Build the canonical byte string. Validates input invariants (username
    /// length ≤ 255, nonce non-zero) before serialization.
    pub fn build(inputs: AuthMessageInputs<'_>) -> Result<Self> {
        let username = inputs.username.as_bytes();
        if username.len() > limits::USERNAME_MAX_BYTES {
            return Err(Error::InvalidUsername("> 255 bytes after NFC"));
        }
        if inputs.client_nonce.iter().all(|&b| b == 0) {
            return Err(Error::InvalidInput("client_nonce all-zero"));
        }
        if inputs.server_nonce.iter().all(|&b| b == 0) {
            return Err(Error::InvalidInput("server_nonce all-zero"));
        }

        // Pre-allocate exact size where possible (header 14 + len2 + var + 32+32+16
        // + 4+4+4 + 1+1+1 + 32 + 1 = 142 + username_len).
        let mut bytes = Vec::with_capacity(142 + username.len());

        bytes.extend_from_slice(AUTH_V1); // 14
        bytes.extend_from_slice(&(username.len() as u16).to_be_bytes()); // 2
        bytes.extend_from_slice(username); // var
        bytes.extend_from_slice(inputs.client_nonce); // 32
        bytes.extend_from_slice(inputs.server_nonce); // 32
        bytes.extend_from_slice(inputs.salt); // 16
        bytes.extend_from_slice(&inputs.kdf_params.memory_kb.to_be_bytes()); // 4
        bytes.extend_from_slice(&inputs.kdf_params.time.to_be_bytes()); // 4
        bytes.extend_from_slice(&inputs.kdf_params.parallelism.to_be_bytes()); // 4
        bytes.push(inputs.kdf_params.argon2_version); // 1
        bytes.push(inputs.transport_kind.as_u8()); // 1
        bytes.push(inputs.binding_mode.as_u8()); // 1
        bytes.extend_from_slice(inputs.tls_exporter_or_zeros); // 32
        bytes.push(inputs.supported_version.as_u8()); // 1

        Ok(Self { bytes })
    }

    /// Underlying byte string (input to HMAC / Ed25519).
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Total length (149 bytes for default 5-char username + default kdf params).
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Empty-check (always false since header alone is 14 bytes).
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}
