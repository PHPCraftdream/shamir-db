//! Server-side configuration: per-listener policies and global secrets.

use crate::common::types::BindingMode;

/// Per-listener policy describing the **single** acceptable `binding_mode`.
///
/// Server enforces this BEFORE running Argon2id to defeat DoS amplification
/// (spec §4.3 [NORMATIVE]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerPolicy {
    /// Allowed binding mode for this listener. Anything else → silent close.
    pub binding_mode: BindingMode,
}

impl ListenerPolicy {
    /// Construct.
    pub const fn new(binding_mode: BindingMode) -> Self {
        Self { binding_mode }
    }
}

/// Global server secrets — held in memory, persisted to `__system__/server_meta`
/// on real deployments.
#[derive(Debug, Clone)]
pub struct ServerSecrets {
    /// Anti-enumeration HKDF IKM (rotated on schedule).
    pub server_secret: [u8; 32],
    /// Per-spec §5.2.5: SEPARATE secret for `username_hash`. NOT rotated.
    pub lockout_secret: [u8; 32],
}
