//! Bootstrap signature payload (spec §11.3.3).
//!
//! Server signs and client verifies. Layout:
//!
//! ```text
//! identity_sig_bootstrap = Ed25519::sign(server_priv,
//!   "SHAMIR-BOOTSTRAP-v1"
//!   || SHA256(server_pub_key)
//!   || u8(transport_kind)
//!   || tls_exporter(32)
//!   || client_nonce(32)
//!   || u64_be(server_time_ns)
//! )
//! ```

use crate::common::crypto::sha256;
use crate::common::domain_tags::BOOTSTRAP_V1;
use crate::common::types::TransportKind;

/// Build the canonical signing payload for the bootstrap challenge.
pub fn build_bootstrap_input(
    server_pub_key: &[u8; 32],
    transport_kind: TransportKind,
    tls_exporter: &[u8; 32],
    client_nonce: &[u8; 32],
    server_time_ns: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(BOOTSTRAP_V1.len() + 32 + 1 + 32 + 32 + 8);
    out.extend_from_slice(BOOTSTRAP_V1);
    out.extend_from_slice(&sha256(server_pub_key));
    out.push(transport_kind.as_u8());
    out.extend_from_slice(tls_exporter);
    out.extend_from_slice(client_nonce);
    out.extend_from_slice(&server_time_ns.to_be_bytes());
    out
}
