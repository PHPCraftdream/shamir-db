//! Validated construction options for an HTTP login service.

use super::super::admission::AdmissionState;
use super::super::pending::PendingStore;
use super::audit::LoginAudit;
use super::issuance::UnixClock;
use crate::config::TokensConfig;
use crate::security::tokens::TokenService;
use crate::security::user_directory::FjallUserDirectory;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::lockout::InMemoryLockoutStore;
use std::sync::Arc;

/// Arguments for validated construction, grouped into one options value.
pub struct LoginServiceOptions {
    /// Durable user directory.
    pub users: Arc<FjallUserDirectory>,
    /// Ordinary token issuer.
    pub tokens: Arc<TokenService>,
    /// Native and login shared lockout implementation.
    pub lockout: Arc<InMemoryLockoutStore>,
    /// Cryptographic server secrets.
    pub secrets: Arc<ServerSecrets>,
    /// Current default KDF tuple.
    pub default_kdf: KdfParams,
    /// Enumeration hardening policy.
    pub hardening: bool,
    /// Listener security class.
    pub security_class: u8,
    /// Persistent listener identity.
    pub listener_id: [u8; 16],
    /// Pending handshake table.
    pub pending: Arc<PendingStore>,
    /// Input admission state.
    pub admission: Arc<AdmissionState>,
    /// Global token configuration.
    pub token_config: TokensConfig,
    /// Whether mutations are disabled.
    pub read_only: bool,
    /// Checked Unix clock.
    pub unix_clock: Arc<UnixClock>,
    /// Optional audit producer.
    pub audit: Option<LoginAudit>,
}
