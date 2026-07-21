//! Per-connection orchestration — TLS accept → optional WS upgrade →
//! pre-Argon2id binding-mode check → rate-limit → SCRAM handshake under
//! Argon2 semaphore + latency padding → lockout register/reset →
//! session insert with per-user cap → request loop with
//! `dispatch_request_view` → 5s grace + audit emit on terminal events.
//!
//! This module wires every security primitive defined elsewhere in
//! `shamir-connect` (lockout, rate_limit, argon2_semaphore,
//! latency, audit_chain, ServerHandshake, dispatch_request_view).

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::lockout::LockoutStore;
use shamir_connect::server::rate_limit::RateLimiter;
use shamir_connect::server::resume::{ConsumedCounterStore, ResumeConfig};
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::SessionStore;

use crate::server_meta::ServerMetaStore;
use crate::user_directory::FjallUserDirectory;

/// Live shared state passed into [`handle_connection`].
pub struct ConnectionContext {
    pub identity: Arc<ServerIdentityState>,
    /// Mirror of the identity keypair (constructed from same seed as
    /// `identity` at boot). `verify_proof` requires `&Ed25519Keypair`.
    pub(super) identity_keypair_inner: shamir_connect::common::crypto::Ed25519Keypair,
    pub secrets: Arc<ServerSecrets>,
    pub kdf_defaults: KdfParams,
    pub session_store: Arc<SessionStore>,
    pub user_dir: Arc<FjallUserDirectory>,
    pub lockout: Arc<dyn LockoutStore>,
    pub rate_limit: Arc<dyn RateLimiter>,
    pub argon2_sem: Arc<Argon2Semaphore>,
    pub audit: Arc<AuditChainWriter>,
    pub resume_config: Arc<ResumeConfig>,
    pub consumed_counters: Arc<dyn ConsumedCounterStore>,
    pub handler: Arc<dyn RequestHandler>,
    /// Listener-pinned `binding_mode` (0x00 / 0x01 / 0x02) — pre-Argon2id
    /// policy check rejects mismatched client claims.
    pub binding_mode: BindingMode,
    /// Listener-pinned transport kind — encoded into auth_message.
    pub transport_kind: TransportKind,
    /// Listener-pinned KDF override, if any (browser endpoints lower the
    /// floor per `docs/dev-artifacts/roadmap/BROWSER_WASM_PLAN.md`).
    pub kdf_override: Option<KdfParams>,
    /// Maximum wall-clock time to wait for the client's `auth_init` after
    /// the TLS handshake completes. Defends against slow-loris attacks —
    /// a TLS-accepted client that never sends a frame holds a per-connection
    /// task + buffers indefinitely otherwise. Real clients send `auth_init`
    /// within ~50 ms; the default of 5 s is comfortably above network jitter.
    pub auth_init_timeout: Duration,
    /// Maximum idle time on an authenticated connection before the server
    /// closes it (task #616 pt.3). Resets on every frame received on the
    /// request loop. Distinct from `auth_init_timeout` (pre-auth only) and
    /// from any per-request wall-clock timeout — this bounds how long a
    /// session can hold its slot + socket while sending nothing at all.
    /// Default is [`shamir_tunables::instance_defaults::CONN_IDLE_TIMEOUT`].
    pub idle_timeout: Duration,
    /// Maximum number of requests in-flight concurrently on a single
    /// connection. Controls the per-connection semaphore + writer-channel
    /// capacity in [`request_loop`]. `1` gives lock-step semantics;
    /// default is [`shamir_tunables::instance_defaults::CONN_MAX_IN_FLIGHT`].
    pub max_in_flight: usize,
    /// Durable server-meta store — used post-handshake to consume an
    /// outstanding bootstrap token on the first successful login for the
    /// username it was issued to (RI-9 bootstrap-token lifecycle).
    pub meta: Arc<ServerMetaStore>,
}

impl ConnectionContext {
    /// Borrow the current Ed25519 keypair for `verify_proof`. Wrapped here
    /// so the call site can stay short and the shamir-connect API stays
    /// keypair-based.
    pub(super) fn identity_keypair_for_verify(
        &self,
    ) -> &shamir_connect::common::crypto::Ed25519Keypair {
        // ServerIdentityState exposes sign_with_current but not the
        // raw keypair. For this binding we shadow-copy the keypair into
        // ConnectionContext during boot (main.rs constructs both from
        // the same seed). See the field below.
        &self.identity_keypair_inner
    }

    /// Build a ConnectionContext from its fields plus an explicit keypair
    /// reference. The keypair MUST share its seed with `identity` — the
    /// boot path enforces this.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<ServerIdentityState>,
        identity_keypair: shamir_connect::common::crypto::Ed25519Keypair,
        secrets: Arc<ServerSecrets>,
        kdf_defaults: KdfParams,
        session_store: Arc<SessionStore>,
        user_dir: Arc<FjallUserDirectory>,
        lockout: Arc<dyn LockoutStore>,
        rate_limit: Arc<dyn RateLimiter>,
        argon2_sem: Arc<Argon2Semaphore>,
        audit: Arc<AuditChainWriter>,
        resume_config: Arc<ResumeConfig>,
        consumed_counters: Arc<dyn ConsumedCounterStore>,
        handler: Arc<dyn RequestHandler>,
        binding_mode: BindingMode,
        transport_kind: TransportKind,
        kdf_override: Option<KdfParams>,
        auth_init_timeout: Duration,
        idle_timeout: Duration,
        max_in_flight: usize,
        meta: Arc<ServerMetaStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            identity,
            identity_keypair_inner: identity_keypair,
            secrets,
            kdf_defaults,
            session_store,
            user_dir,
            lockout,
            rate_limit,
            argon2_sem,
            audit,
            resume_config,
            consumed_counters,
            handler,
            binding_mode,
            transport_kind,
            kdf_override,
            auth_init_timeout,
            idle_timeout,
            max_in_flight,
            meta,
        })
    }
}
