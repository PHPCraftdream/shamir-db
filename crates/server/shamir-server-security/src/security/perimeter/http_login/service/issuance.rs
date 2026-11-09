//! Shared dependencies and listener policy for HTTP login services.

use super::super::admission::AdmissionState;
use super::super::pending::PendingStore;
use super::audit::LoginAudit;
use super::context::LoginServiceOptions;
use crate::security::tokens::TokenService;
use crate::security::user_directory::FjallUserDirectory;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::lockout::{subnet_of, username_hash, InMemoryLockoutStore, PairKey};
use std::net::IpAddr;
use std::sync::Arc;
use tokio::time::Instant;

/// Checked Unix clock returning nanoseconds, or `None` on failure.
pub type UnixClock = dyn Fn() -> Option<u64> + Send + Sync;

/// Immutable dependencies shared by one configured login listener.
pub struct LoginServiceContext {
    /// User storage; blocking reads are dispatched to Tokio's blocking pool.
    pub(crate) users: Arc<FjallUserDirectory>,
    /// Durable token service.
    pub(crate) tokens: Arc<TokenService>,
    /// Shared native/HTTP pair failure store.
    pub(crate) lockout: Arc<InMemoryLockoutStore>,
    /// Current server secrets.
    pub(crate) secrets: Arc<ServerSecrets>,
    /// Global KDF defaults.
    pub(crate) default_kdf: KdfParams,
    /// Exact-KDF enumeration hardening.
    pub(crate) hardening: bool,
    /// Security class used in protocol transcript.
    pub(crate) security_class: u8,
    /// Stable listener id.
    pub(crate) listener_id: [u8; 16],
    /// Bounded volatile pending table.
    pub(crate) pending: Arc<PendingStore>,
    /// Bounded arrival/credit and input-slot state.
    pub(crate) admission: Arc<AdmissionState>,
    /// Allow read-write login scope.
    pub(crate) login_allow_read_write: bool,
    /// Ordinary self-service setting.
    pub(crate) self_service: bool,
    /// Read-only replica state.
    pub(crate) read_only: bool,
    /// Configured login TTL.
    pub(crate) login_ttl: u32,
    /// Global max token TTL days.
    pub(crate) max_ttl_days: u64,
    /// Checked Unix clock.
    pub(crate) unix_clock: Arc<UnixClock>,
    /// Monotonic clock used to anchor padding.
    pub(crate) monotonic_now: Arc<dyn Fn() -> Instant + Send + Sync>,
    /// Optional bounded audit queue handle.
    pub(crate) audit: Option<LoginAudit>,
}

impl LoginServiceContext {
    /// Validate configuration limits and construct the shared context.
    pub fn new(options: LoginServiceOptions) -> Result<Self, &'static str> {
        if options.security_class > 2
            || !(1..=86_400).contains(&options.token_config.login_ttl)
            || options.token_config.max_ttl_days == 0
        {
            return Err("invalid HTTP login policy");
        }
        Ok(Self {
            users: options.users,
            tokens: options.tokens,
            lockout: options.lockout,
            secrets: options.secrets,
            default_kdf: options.default_kdf,
            hardening: options.hardening,
            security_class: options.security_class,
            listener_id: options.listener_id,
            pending: options.pending,
            admission: options.admission,
            login_allow_read_write: options.token_config.login_allow_read_write,
            self_service: options.token_config.allow_self_service,
            read_only: options.read_only,
            login_ttl: options.token_config.login_ttl,
            max_ttl_days: options.token_config.max_ttl_days,
            unix_clock: options.unix_clock,
            monotonic_now: Arc::new(Instant::now),
            audit: options.audit,
        })
    }

    /// Derive the lockout key for a username and client address.
    pub(crate) fn pair_key(&self, user: &str, source: IpAddr) -> PairKey {
        (
            subnet_of(source),
            username_hash(&self.secrets.lockout_secret, user.as_bytes()),
        )
    }

    /// Effective login token TTL, capped by the global maximum.
    pub(crate) fn ttl_seconds(&self) -> Option<u64> {
        let max_ttl = self.max_ttl_days.checked_mul(86_400)?;
        let ttl = u64::from(self.login_ttl).min(max_ttl);
        (ttl != 0).then_some(ttl)
    }
}
