//! Wire-level type definitions and enums.
//!
//! Per spec §4.2: `transport_kind` and `binding_mode` are `u8` enums
//! embedded in `auth_message`. Unknown values → fail-closed.

use crate::common::error::{Error, Result};

/// Protocol major version.
///
/// Reflected on the wire as `u8` in `auth_init.version` and as the trailing
/// `supported_version` byte of `auth_message` (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolVersion(pub u8);

impl ProtocolVersion {
    /// Current protocol version (v1).
    pub const V1: ProtocolVersion = ProtocolVersion(1);

    /// Wire-byte representation.
    pub const fn as_u8(self) -> u8 {
        self.0
    }
}

/// Transport kind tag — embedded in `auth_message` (spec §4.2).
///
/// Server enforces listener policy: each listener accepts only specific
/// transport+binding combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TransportKind {
    /// TCP transport (spec TRANSPORT_TCP.md).
    Tcp = 0x01,
    /// WebSocket transport (spec TRANSPORT_WS.md).
    WebSocket = 0x02,
}

impl TransportKind {
    /// Wire byte.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parse from wire byte. Unknown → fail-closed per spec §4.2 enum extension rule.
    pub fn from_u8(byte: u8) -> Result<Self> {
        match byte {
            0x01 => Ok(Self::Tcp),
            0x02 => Ok(Self::WebSocket),
            _ => Err(Error::InvalidInput("unknown transport_kind")),
        }
    }
}

/// Binding mode tag — embedded in `auth_message` (spec §4.2).
///
/// Determines how `tls_exporter_or_zeros` is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BindingMode {
    /// No TLS (plain transport — loopback only per spec).
    /// `tls_exporter_or_zeros = bytes(32) all zeros`.
    None = 0x00,
    /// TLS present, exporter extracted by both sides.
    /// `tls_exporter_or_zeros = TLS-Exporter(label="EXPORTER-ShamirDB-AUTH-v1", ctx="", L=32)`.
    TlsExporter = 0x01,
    /// TLS present but client lacks exporter API (browser path).
    /// `tls_exporter_or_zeros = bytes(32) all zeros` (weakened binding).
    TlsNoExport = 0x02,
}

impl BindingMode {
    /// Wire byte.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parse from wire byte. Unknown → fail-closed.
    pub fn from_u8(byte: u8) -> Result<Self> {
        match byte {
            0x00 => Ok(Self::None),
            0x01 => Ok(Self::TlsExporter),
            0x02 => Ok(Self::TlsNoExport),
            _ => Err(Error::InvalidInput("unknown binding_mode")),
        }
    }

    /// Strength ordering for resumption anti-downgrade rule (spec SESSION_RESUMPTION §6.1).
    ///
    /// Higher = stronger. Resume rejected if `strength(now) < strength(at_auth)`.
    pub const fn strength(self) -> u8 {
        match self {
            Self::None => 0,
            Self::TlsNoExport => 1,
            Self::TlsExporter => 2,
        }
    }
}

/// Length constants from spec §8.
pub mod limits {
    /// Pre-auth frame ceiling (spec §8).
    pub const MAX_PRE_AUTH_FRAME: usize = 4 * 1024;
    /// Post-auth data frame ceiling (server tunable).
    pub const MAX_FRAME_SIZE_DATA: usize = 16 * 1024 * 1024;
    /// Username max bytes after NFC + UsernameCaseMapped.
    pub const USERNAME_MAX_BYTES: usize = 255;
    /// Recommended username soft cap for UX.
    pub const USERNAME_SOFT_LIMIT_BYTES: usize = 64;

    /// Password char (UTF-8 char) lower bound (spec §3.2).
    pub const PASSWORD_MIN_CHARS: usize = 12;
    /// Password char upper bound.
    pub const PASSWORD_MAX_CHARS: usize = 1024;

    /// Argon2id hard caps validated by client before launching KDF (spec §5.1.1).
    pub const KDF_MAX_MEMORY_KB: u32 = 262_144; // 256 MB
    /// Hard cap on time parameter (Argon2 passes).
    pub const KDF_MAX_TIME: u32 = 8;
    /// Hard cap on parallelism lanes.
    pub const KDF_MAX_PARALLEL: u32 = 8;

    /// Argon2id minimum floor (server config rejects below — spec §3.7.2).
    pub const KDF_MIN_MEMORY_KB: u32 = 19_456; // OWASP min
    /// Min time.
    pub const KDF_MIN_TIME: u32 = 2;
    /// Min parallelism.
    pub const KDF_MIN_PARALLELISM: u32 = 1;

    /// Nonce sizes.
    pub const CLIENT_NONCE_BYTES: usize = 32;
    /// Server nonce size.
    pub const SERVER_NONCE_BYTES: usize = 32;
    /// Salt size (Argon2id input).
    pub const SALT_BYTES: usize = 16;
    /// `session_id` size.
    pub const SESSION_ID_BYTES: usize = 32;
    /// Ticket family identifier size (per-device lineage).
    pub const TICKET_FAMILY_ID_BYTES: usize = 16;

    /// Argon2id v1.3 byte (RFC 9106).
    pub const ARGON2_VERSION_V13: u8 = 0x13;
}
