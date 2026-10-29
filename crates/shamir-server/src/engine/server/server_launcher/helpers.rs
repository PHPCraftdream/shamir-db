//! Private helpers shared by `ServerLauncher::launch` and the per-transport
//! accept loops: listener-path dispatch, connection-context construction,
//! and the bootstrap/identity logging + derivation utilities.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::crypto::Ed25519Keypair;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::resume::{InMemoryConsumedCounters, ResumeConfig};
use shamir_connect::server::session::SessionStore;

use shamir_transport_ws::browser::BrowserOriginPolicy;

use shamir_db::access::Actor;
use shamir_db::ShamirDb;

use crate::connection::{ConnTasks, ConnectionContext};
use crate::server::boot_error::BootError;
use crate::server_meta::ServerMetaStore;
use crate::user_directory::FjallUserDirectory;

use crate::bootstrap::BootstrapOutcome;

/// Per-listener loop dispatch — picked at config-validation time, used at
/// boot to decide which `accept_loop_*` to spawn.
pub(super) enum AcceptPath {
    /// `kind=tcp + profile=tls_exporter`: native binding_mode 0x01.
    TcpTlsExporter,
    /// `kind=tcp + profile=plain`: no TLS, binding_mode 0x00. Config
    /// validation (`Config::validate`) already restricts this to loopback
    /// addresses; `bind_tcp`'s own `ListenerProfile::Plain` policy check is
    /// a second, defense-in-depth enforcement of the same rule at bind time.
    TcpPlain,
    /// `kind=ws + profile=tls_exporter`: native WSS, binding_mode 0x01.
    WsNative,
    /// `kind=ws + profile=tls_no_export`: browser WSS, binding_mode 0x02
    /// with mandatory Origin allowlist.
    WsBrowser(BrowserOriginPolicy),
}

/// Build a `BrowserOriginPolicy` from the listener's `browser_origin_allowlist`.
/// Empty allowlist is rejected at config validation, so by the time we get
/// here we always have at least one origin.
pub(super) fn browser_origin_policy_from(
    allowlist: &[String],
) -> Result<BrowserOriginPolicy, BootError> {
    if allowlist.is_empty() {
        return Err(BootError::Bind(
            "browser_origin_allowlist must not be empty for tls_no_export ws listeners".into(),
        ));
    }
    BrowserOriginPolicy::try_allow(allowlist.iter().cloned()).map_err(BootError::Bind)
}

/// Build a per-listener `ConnectionContext`. Each listener gets its own
/// `Ed25519Keypair` reconstructed from the same identity seed — this is a
/// workaround for shamir-connect's `verify_proof` API requiring
/// `&Ed25519Keypair` while `ServerIdentityState` doesn't expose the keypair.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_ctx(
    identity: &Arc<shamir_connect::server::rotation::ServerIdentityState>,
    identity_seed: [u8; 32],
    secrets: &Arc<ServerSecrets>,
    kdf: KdfParams,
    session_store: &Arc<SessionStore>,
    user_dir: &Arc<FjallUserDirectory>,
    lockout: Arc<dyn shamir_connect::server::lockout::LockoutStore>,
    rate_limit: Arc<dyn shamir_connect::server::rate_limit::RateLimiter>,
    argon2_sem: &Arc<Argon2Semaphore>,
    audit_writer: &Arc<AuditChainWriter>,
    resume_config: &Arc<ResumeConfig>,
    handler: &Arc<dyn shamir_connect::server::dispatch::RequestHandler>,
    binding_mode: BindingMode,
    transport_kind: TransportKind,
    kdf_override: Option<KdfParams>,
    auth_init_timeout: Duration,
    meta: &Arc<ServerMetaStore>,
    tasks: ConnTasks,
) -> Arc<ConnectionContext> {
    let identity_keypair = Ed25519Keypair::from_seed(&identity_seed);
    let max_in_flight = shamir_tunables::instance_defaults::CONN_MAX_IN_FLIGHT;
    let idle_timeout = shamir_tunables::instance_defaults::CONN_IDLE_TIMEOUT;
    ConnectionContext::new(
        identity.clone(),
        identity_keypair,
        secrets.clone(),
        kdf,
        session_store.clone(),
        user_dir.clone(),
        lockout,
        rate_limit,
        argon2_sem.clone(),
        audit_writer.clone(),
        resume_config.clone(),
        Arc::new(InMemoryConsumedCounters::new()),
        handler.clone(),
        binding_mode,
        transport_kind,
        kdf_override,
        auth_init_timeout,
        idle_timeout,
        max_in_flight,
        meta.clone(),
        tasks,
    )
}

/// Run one connection task until it ends or the server shuts down. Dropping the
/// connection future releases its limiter guards and every store reference
/// reachable through its handler context, which is what lets
/// `ServerHandle::shutdown` return with the stores released.
/// Post-handshake teardown is shielded by request_loop's own shutdown branch.
pub(super) async fn run_until_cancelled<F: std::future::Future<Output = ()>>(
    shutdown: tokio_util::sync::CancellationToken,
    connection: F,
) {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => {}
        _ = connection => {}
    }
}

/// Loopback placeholder `peer_addr` for local-IPC connections — a Unix
/// socket / Named Pipe has no real peer IP, but `handle_connection` keys
/// its per-subnet rate-limit / lockout state off `peer_addr.ip()`.
/// Consequence (accepted for this transport): every simultaneous local-IPC
/// connection shares ONE per-IP `ConnLimiter`/`PerIpLimiter` bucket rather
/// than getting per-peer isolation — reasonable given they are inherently
/// same-host and already gated by the OS-level access boundary (spec
/// TRANSPORT_UNIX.md §5), not something that needs per-peer distinction.
pub(super) const IPC_PEER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// How often the follower supervisor re-reads the subscription catalogue and
/// converges its running loops. Event-driven convergence is future work (see
/// the TODO at the tick spawn site); until then this bounds the staleness of
/// a subscription change to at most one interval.
pub(super) const REPL_RECONCILE_INTERVAL: Duration = Duration::from_secs(10);

/// Derive a stable-enough follower `node_id` when none is configured: the
/// machine hostname (from `COMPUTERNAME` on Windows / `HOSTNAME` elsewhere),
/// falling back to a boot-nanos-derived id. Advertised in `ReplHello` (§5.2).
///
/// No new dependency is pulled for hostname resolution — the env var is the
/// portable minimal path; a config `node_id` is the recommended production
/// setting anyway (this is only the unconfigured fallback).
pub(super) fn default_node_id() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let nanos = UnixNanos::now().as_u64();
            format!("follower-{nanos:016x}")
        })
}

pub(super) fn kdf_from_config(c: &crate::config::KdfConfig) -> KdfParams {
    KdfParams {
        memory_kb: c.memory_kb,
        time: c.time,
        parallelism: c.parallelism,
        argon2_version: c.argon2_version,
    }
}

pub(super) fn log_bootstrap_outcome(name: &str, outcome: &BootstrapOutcome) {
    match outcome {
        BootstrapOutcome::AlreadyExists => {
            tracing::debug!(user = %name, "bootstrap: user already exists");
        }
        BootstrapOutcome::Created { token: None, .. } => {
            tracing::warn!(
                user = %name,
                "bootstrap: superuser created with operator-supplied password",
            );
        }
        BootstrapOutcome::Created {
            token: Some(_tok),
            token_path,
        } => {
            // SECURITY (M-tier audit M4): only log the *path* to the
            // token file, never the token itself. Logged tokens persist
            // in journald / k8s log aggregation indefinitely; the file
            // is permission-protected (0o600 on Unix) and is the
            // legitimate retrieval channel. Once the operator changes
            // the password, the file should be deleted so this branch
            // never fires again (Created only happens once).
            tracing::warn!(
                user = %name,
                token_path = ?token_path,
                "bootstrap: superuser created with one-time token — \
                 READ THE TOKEN FROM THE FILE, LOG IN, CHANGE PASSWORD, \
                 THEN DELETE THE TOKEN FILE",
            );
        }
    }
    // Suppress dead-code in non-tracing builds (defensive).
    let _ = ServerSecrets {
        server_secret: [0u8; 32],
        lockout_secret: [0u8; 32],
    };
}

/// Boot-owned step: idempotently ensure the `default` database exists and
/// return its live instance.
///
/// On the first boot this runs `create_db_as_checked`, whose catalogue
/// persist failure PROPAGATES (RV-DB-6540DC7B: boot must not continue with
/// a `default` db whose durable row failed to save — the checked creation
/// persists BEFORE publishing the live registration, so a failed call
/// leaves no false-success live default either). On every subsequent boot
/// `ShamirDb::init` already loaded `default` from the system store and the
/// existing instance is returned untouched.
pub(super) async fn ensure_default_database(
    shamir: &Arc<ShamirDb>,
) -> Result<shamir_db::engine::db_instance::db_instance::DbInstance, BootError> {
    if !shamir.has_db("default") {
        shamir
            .create_db_as_checked("default", Actor::System)
            .await
            .map_err(|e| BootError::ShamirDbInit(format!("create_db default: {e}")))?;
    }
    // Cannot miss after the branch above: create_db_as_checked registers
    // the instance before returning Ok, and has_db(true) implies a live
    // registration.
    shamir
        .get_db("default")
        .ok_or_else(|| BootError::ShamirDbInit("default db missing after ensure".into()))
}
