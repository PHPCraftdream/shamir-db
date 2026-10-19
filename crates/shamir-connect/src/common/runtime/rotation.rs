//! Identity rotation payloads (spec §6.5, §12.2).
//!
//! Two distinct signed payloads:
//! - `signed_by_old` = Ed25519 over `SHAMIR-ROTATE-v1 || SHA256(old_pub) ||
//!   new_pub || u64_be(transition_until_ns) || recipient_session_id(32)` —
//!   broadcast to **active** sessions (one per recipient).
//! - `rotation_proof` = Ed25519 over `SHAMIR-ROTATE-PROOF-v1 ||
//!   SHA256(previous_pub) || current_pub || u64_be(transition_until_ns)` —
//!   embedded in `auth_ok.rotation_in_progress` for **offline** clients
//!   doing fresh SCRAM (orphan recovery, spec §6.5).

use crate::common::crypto::sha256;
use crate::common::domain_tags::{ROTATE_PROOF_V1, ROTATE_V1};

/// Build the broadcast `signed_by_old` payload (spec §12.2 step 4).
pub fn build_rotate_event_input(
    old_pub: &[u8; 32],
    new_pub: &[u8; 32],
    transition_until_ns: u64,
    recipient_session_id: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(ROTATE_V1.len() + 32 + 32 + 8 + 32);
    out.extend_from_slice(ROTATE_V1);
    out.extend_from_slice(&sha256(old_pub));
    out.extend_from_slice(new_pub);
    out.extend_from_slice(&transition_until_ns.to_be_bytes());
    out.extend_from_slice(recipient_session_id);
    out
}

/// Build the orphan-recovery `rotation_proof` payload (spec §6.5).
pub fn build_rotation_proof_input(
    previous_pub: &[u8; 32],
    current_pub: &[u8; 32],
    transition_until_ns: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(ROTATE_PROOF_V1.len() + 32 + 32 + 8);
    out.extend_from_slice(ROTATE_PROOF_V1);
    out.extend_from_slice(&sha256(previous_pub));
    out.extend_from_slice(current_pub);
    out.extend_from_slice(&transition_until_ns.to_be_bytes());
    out
}
