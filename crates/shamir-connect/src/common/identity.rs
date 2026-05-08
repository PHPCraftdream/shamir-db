//! Server identity payload (`identity_input`) and Ed25519 binding.
//!
//! Per spec §5.2.4 the server signs with its `server_ed25519_priv` over:
//!
//! ```text
//! identity_input = "SHAMIR-IDENTITY-v1"
//!               || SHA256(server_pub_key)
//!               || u8(transport_kind)
//!               || u8(binding_mode)
//!               || tls_exporter_or_zeros(32)
//!               || auth_message
//!               || session_id(32)
//!               || u64_be(expires_at_ns)
//! identity_sig = Ed25519::sign(server_priv, identity_input)
//! ```
//!
//! The client recomputes `identity_input` from received fields and runs
//! `verify_strict(server_pub_key, identity_input, identity_sig)` per spec §5.3.

use crate::common::auth_message::AuthMessage;
use crate::common::crypto::{ed25519_verify_strict, sha256, Ed25519Keypair};
use crate::common::domain_tags::IDENTITY_V1;
use crate::common::types::{BindingMode, TransportKind};

/// Build the canonical `identity_input` byte string for signing/verification.
pub fn build_identity_input(
    server_pub_key: &[u8; 32],
    transport_kind: TransportKind,
    binding_mode: BindingMode,
    tls_exporter_or_zeros: &[u8; 32],
    auth_message: &AuthMessage,
    session_id: &[u8; 32],
    expires_at_ns: u64,
) -> Vec<u8> {
    let am_bytes = auth_message.as_bytes();
    let mut out = Vec::with_capacity(
        IDENTITY_V1.len()    // 18
        + 32                 // SHA256(server_pub_key)
        + 1 + 1              // transport_kind, binding_mode
        + 32                 // tls_exporter_or_zeros
        + am_bytes.len()
        + 32                 // session_id
        + 8, // expires_at_ns
    );
    out.extend_from_slice(IDENTITY_V1);
    out.extend_from_slice(&sha256(server_pub_key));
    out.push(transport_kind.as_u8());
    out.push(binding_mode.as_u8());
    out.extend_from_slice(tls_exporter_or_zeros);
    out.extend_from_slice(am_bytes);
    out.extend_from_slice(session_id);
    out.extend_from_slice(&expires_at_ns.to_be_bytes());
    out
}

/// Server: sign the `identity_input` with the server Ed25519 priv key.
pub fn sign_identity(keypair: &Ed25519Keypair, identity_input: &[u8]) -> [u8; 64] {
    keypair.sign(identity_input)
}

/// Client: verify `identity_sig` against `server_pub_key` over the recomputed
/// `identity_input`. Strict mode per RFC 8032 §6 (small-subgroup rejection).
pub fn verify_identity(
    server_pub_key: &[u8; 32],
    identity_input: &[u8],
    identity_sig: &[u8; 64],
) -> bool {
    ed25519_verify_strict(server_pub_key, identity_input, identity_sig)
}
