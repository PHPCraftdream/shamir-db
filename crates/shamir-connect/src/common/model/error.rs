//! Error types for the connection protocol.
//!
//! Errors are deliberately structured but follow the spec's privacy rules:
//! anything sent on the wire to a peer collapses to a generic
//! [`AuthFailed`](Error::AuthFailed). Internal variants exist for callers
//! that need to log / audit / branch on real cause.

use thiserror::Error;

/// Result alias for connection protocol operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Connection protocol errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Generic authentication failure — what the wire sees.
    /// All real failure causes (unknown user, bad password, bad proof, lockout)
    /// collapse to this in client-visible responses (spec §14.1).
    #[error("authentication_failed")]
    AuthFailed,

    /// Server proof verification failed on the client side (spec §14.2).
    #[error("server_authentication_failed")]
    ServerAuthFailed,

    /// Ed25519 identity signature verification failed (spec §14.2).
    #[error("server_signature_invalid")]
    ServerSignatureInvalid,

    /// TOFU pin mismatch — server identity does not match pinned hash (§14.2).
    #[error("server_identity_changed")]
    ServerIdentityChanged,

    /// KDF parameters from server exceed local hard limits (spec §5.1.1, §14.2).
    #[error("kdf_params_rejected")]
    KdfParamsRejected,

    /// known_hosts file integrity tag mismatch (spec §14.2).
    #[error("known_hosts_integrity_failed")]
    KnownHostsIntegrityFailed,

    /// Rate limit hit (server-side response, spec §14.3).
    #[error("rate_limited")]
    RateLimited {
        /// Recommended retry delay in seconds.
        retry_after: u32,
    },

    /// Server too busy (Argon2id semaphore exhausted, §14.3).
    #[error("server_busy")]
    ServerBusy {
        /// Recommended retry delay.
        retry_after: u32,
    },

    /// Protocol version negotiation failed (§14.3).
    #[error("unsupported_version")]
    UnsupportedVersion,

    /// Bootstrap handshake failed — generic (§14.3).
    #[error("bootstrap_failed")]
    BootstrapFailed,

    /// Invalid input violating spec invariants (e.g. nonce wrong length,
    /// username too long after NFC, all-zeros nonce).
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),

    /// Username failed PRECIS UsernameCaseMapped + NFC validation (spec §15.3).
    #[error("invalid username: {0}")]
    InvalidUsername(&'static str),

    /// Password violates policy — length out of range, forbidden pattern (§3.2).
    #[error("invalid password: {0}")]
    InvalidPassword(&'static str),

    /// Crypto primitive error (Argon2id failure, AES-GCM tag mismatch, etc).
    #[error("crypto: {0}")]
    Crypto(&'static str),

    /// Serialization / deserialization failure.
    #[error("encoding: {0}")]
    Encoding(String),
}

impl Error {
    /// Returns the wire-safe variant: any internal failure cause is collapsed
    /// to [`Self::AuthFailed`] for client-visible responses (spec §14.4).
    pub fn to_wire(&self) -> Error {
        match self {
            Error::RateLimited { retry_after } => Error::RateLimited {
                retry_after: *retry_after,
            },
            Error::ServerBusy { retry_after } => Error::ServerBusy {
                retry_after: *retry_after,
            },
            Error::UnsupportedVersion => Error::UnsupportedVersion,
            Error::BootstrapFailed => Error::BootstrapFailed,
            // Everything else (incl. internal cause) → generic.
            _ => Error::AuthFailed,
        }
    }
}
