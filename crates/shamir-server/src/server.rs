//! Boot orchestration — extracted from `main.rs` so it is reusable from
//! integration tests.
//!
//! [`ServerLauncher`] owns the [`Config`] + bootstrap policy and produces
//! a [`ServerHandle`] when launched. The handle holds the bound listener
//! addresses (so test code can connect a real client to them) plus
//! shutdown plumbing for the listener tasks, the background scheduler,
//! and the audit appender.
//!
//! The launcher does NOT install the rustls crypto provider — callers
//! must do that exactly once per process (`rustls::crypto::aws_lc_rs::default_provider().install_default()`).
//! This is enforced by rustls itself: a second install is a no-op.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::crypto::Ed25519Keypair;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::audit_chain::{AuditAppender, AuditChain, AuditChainWriter};
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::durable_counters::RedbConsumedCounters;
use shamir_connect::server::lockout::InMemoryLockoutStore;
use shamir_connect::server::rate_limit::InMemoryRateLimiter;
use shamir_connect::server::resume::ResumeConfig;
use shamir_connect::server::session::SessionStore;

use shamir_db::db::ShamirDb;

use shamir_transport_tcp::listener::{bind_validated, ListenerProfile};

use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use zeroize::Zeroizing;

use crate::audit_appender::RedbAuditAppender;
use crate::bootstrap::{
    ensure_superuser, BootstrapOutcome, BootstrapPolicy, DEFAULT_BOOTSTRAP_NAME,
};
use crate::config::{Config, ListenerKind, ProfileKind};
use crate::connection::{handle_connection, ConnectionContext};
use crate::db_handler::{AdminGlue, ShamirDbHandler};
use crate::scheduler::{Scheduler, SchedulerConfig, SchedulerInputs};
use crate::server_meta::ServerMetaStore;
use crate::tls::{load_or_generate, subject_alts_from_addrs, LoadedTls};
use crate::user_directory::RedbUserDirectory;

/// Bootstrap policy options exposed at boot time.
pub enum BootstrapMode {
    /// Use the supplied password verbatim.
    Password {
        username: String,
        password: Zeroizing<Vec<u8>>,
    },
    /// Generate a 32-byte random token (printed to logs + written to
    /// `data_dir/bootstrap_token.txt`). Username defaults to `admin`.
    RandomToken {
        /// Optional override of the default `admin` username.
        username: Option<String>,
    },
    /// Skip bootstrap entirely; assume the directory already has a
    /// superuser. Used when the operator manages users out-of-band.
    Skip,
}

impl Default for BootstrapMode {
    fn default() -> Self {
        BootstrapMode::RandomToken { username: None }
    }
}

/// Errors that can happen during boot.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("config: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("server_meta: {0}")]
    ServerMeta(String),
    #[error("user_directory: {0}")]
    UserDirectory(String),
    #[error("counters: {0}")]
    Counters(String),
    #[error("audit_appender: {0}")]
    AuditAppender(String),
    #[error("shamir_db init: {0}")]
    ShamirDbInit(String),
    #[error("tls: {0}")]
    Tls(#[from] crate::tls::TlsError),
    #[error("bootstrap: {0}")]
    Bootstrap(#[from] crate::bootstrap::BootstrapError),
    #[error("listener bind: {0}")]
    Bind(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Owner of the runtime state of a launched server.
pub struct ServerHandle {
    /// Addresses the server actually bound, in the same order as
    /// `config.listeners`. `None` entries correspond to skipped listeners
    /// (WS / plain are not yet supported by this MVP boot path).
    pub bound_addrs: Vec<Option<SocketAddr>>,
    /// Per-listener accept-loop join handles.
    listener_tasks: Vec<JoinHandle<()>>,
    /// Background task scheduler.
    scheduler: Scheduler,
    /// Audit-log appender (drained on shutdown).
    audit_appender: Arc<RedbAuditAppender>,
    /// Notify that signals all accept loops to stop.
    shutdown_notify: Arc<Notify>,
}

impl ServerHandle {
    /// Stop accepting new connections, then drain the scheduler + audit log.
    /// In-flight per-connection tasks are NOT awaited explicitly — they
    /// finish on their own once their TcpStreams close.
    pub async fn shutdown(self) {
        // 1. Tell all accept loops to stop.
        self.shutdown_notify.notify_waiters();
        // 2. Wait for them to finish — they exit promptly since
        //    `select!` on `notify.notified()` short-circuits.
        for task in self.listener_tasks {
            let _ = task.await;
        }
        // 3. Drain the audit chain + scheduler.
        self.audit_appender.shutdown().await;
        self.scheduler.shutdown().await;
    }

    /// Returns the first bound TCP+TLS-exporter address — useful for
    /// integration tests that just want "where do I connect?".
    pub fn first_tls_exporter_addr(&self) -> Option<SocketAddr> {
        self.bound_addrs.iter().filter_map(|a| *a).next()
    }
}

/// Launcher: build the server runtime from a [`Config`] + bootstrap policy.
pub struct ServerLauncher {
    pub config: Config,
    pub bootstrap: BootstrapMode,
}

impl ServerLauncher {
    /// New launcher with the given config and the default bootstrap mode
    /// (random token, written to `data_dir/bootstrap_token.txt`).
    pub fn new(config: Config) -> Self {
        Self {
            config,
            bootstrap: BootstrapMode::default(),
        }
    }

    /// Run the boot sequence — everything from "open the redb files" to
    /// "spawn the accept loops". Returns when all listeners are bound but
    /// before any connection arrives.
    pub async fn launch(self) -> Result<ServerHandle, BootError> {
        let ServerLauncher { config, bootstrap } = self;
        config.validate()?;

        // 1. Durable stores.
        std::fs::create_dir_all(&config.data_dir)?;

        let meta = ServerMetaStore::open_or_init(config.data_dir.join("server_meta.redb"))
            .map_err(|e| BootError::ServerMeta(e.to_string()))?;

        let user_dir = Arc::new(
            RedbUserDirectory::open(config.data_dir.join("users.redb"))
                .map_err(|e| BootError::UserDirectory(e.to_string()))?,
        );
        let counters = Arc::new(
            RedbConsumedCounters::open(config.data_dir.join("counters.redb"))
                .map_err(|e| BootError::Counters(e.to_string()))?,
        );

        let audit_appender = RedbAuditAppender::open_batched(
            &config.data_dir,
            std::time::Duration::from_secs(5),
        )
        .map_err(|e| BootError::AuditAppender(e.to_string()))?;

        // 2. Bootstrap (idempotent).
        let kdf_for_bootstrap = kdf_from_config(&config.kdf_defaults);
        match &bootstrap {
            BootstrapMode::Password { username, password } => {
                let outcome = ensure_superuser(
                    &user_dir,
                    &config.data_dir,
                    username,
                    BootstrapPolicy::Password(password),
                    &kdf_for_bootstrap,
                )?;
                log_bootstrap_outcome(username, &outcome);
            }
            BootstrapMode::RandomToken { username } => {
                let name = username.as_deref().unwrap_or(DEFAULT_BOOTSTRAP_NAME);
                let outcome = ensure_superuser(
                    &user_dir,
                    &config.data_dir,
                    name,
                    BootstrapPolicy::RandomToken,
                    &kdf_for_bootstrap,
                )?;
                log_bootstrap_outcome(name, &outcome);
            }
            BootstrapMode::Skip => {
                tracing::debug!("bootstrap: skipped per BootstrapMode::Skip");
            }
        }

        // 3. In-memory stores.
        let lockout = Arc::new(InMemoryLockoutStore::new());
        let now_ns = UnixNanos::now().as_u64();
        let rate_limit = Arc::new(InMemoryRateLimiter::new(now_ns));
        let argon2_sem = Arc::new(Argon2Semaphore::with_capacity(config.argon2_concurrent_max));
        let session_store = Arc::new(SessionStore::new());

        // 4. Audit chain — load from checkpoint if present.
        let audit_chain = match meta.audit_checkpoint() {
            Some((seq, hmac)) => {
                Arc::new(AuditChain::from_checkpoint(meta.audit_chain_key(), seq, hmac))
            }
            None => Arc::new(AuditChain::new(meta.audit_chain_key())),
        };
        let audit_writer = Arc::new(AuditChainWriter::new(
            AuditChain::new(meta.audit_chain_key()),
            audit_appender.clone() as Arc<dyn AuditAppender>,
        ));

        // 5. ShamirDb (in-memory for v1; durable persistence is a follow-up).
        //    Create a `default` database eagerly so wire-side admin batches
        //    have a target for `create_db`/`drop_db` ops without needing an
        //    out-of-band ShamirDb handle. (`ShamirDb::execute` requires the
        //    target `db_name` to already exist for resolver lookup.)
        let shamir = Arc::new(
            ShamirDb::init_memory()
                .await
                .map_err(|e| BootError::ShamirDbInit(e.to_string()))?,
        );
        let _ = shamir.create_db("default").await;
        let handler: Arc<dyn RequestHandler> = Arc::new(ShamirDbHandler::with_admin(
            shamir.clone(),
            AdminGlue {
                user_dir: user_dir.clone(),
                kdf: kdf_for_bootstrap,
            },
        ));

        // 6. ResumeConfig.
        let (current_key, previous_key) = meta.ticket_keys();
        let resume_config = Arc::new(ResumeConfig::new(
            current_key,
            previous_key,
            true,  // allow_browser_ticket_upgrade
            false, // disable_plain_ticket_upgrade
        ));

        // 7. Identity material.
        let identity = Arc::new(meta.identity_state());
        let identity_seed = meta.current_identity_seed();
        let secrets = Arc::new(meta.server_secrets());

        // 8. Scheduler.
        let scheduler = Scheduler::spawn(
            SchedulerInputs {
                counters: counters.clone(),
                lockout: lockout.clone(),
                rate_limit: rate_limit.clone(),
                session_store: session_store.clone(),
                session_max_age_ns: 24 * shamir_connect::common::time::ns::HOUR,
                session_idle_ttl_ns: 30 * shamir_connect::common::time::ns::MINUTE,
                audit_chain: audit_chain.clone(),
                audit_appender: audit_appender.clone(),
                identity: identity.clone(),
            },
            SchedulerConfig::default_for_production(),
        );

        // 9. TLS material — load or generate before binding any TCP+TLS
        //    listener so we can fail fast on missing PEMs.
        //
        // We generate SAN entries lazily for now — the cert covers
        // `localhost` and every literal IP in the configured listeners.
        // Self-signed cert chain is informational only (Ed25519 pin in
        // `auth_ok` is the real identity check).
        let parsed_addrs: Vec<SocketAddr> = config
            .listeners
            .iter()
            .filter_map(|l| l.addr.parse::<SocketAddr>().ok())
            .collect();
        let LoadedTls { server_config: tls_server, generated } = load_or_generate(
            &config.tls.cert_path,
            &config.tls.key_path,
            subject_alts_from_addrs(&parsed_addrs),
        )?;
        if generated {
            tracing::warn!(
                cert_path = ?config.tls.cert_path,
                "tls: self-signed cert generated — clients pin the Ed25519 \
                 identity, the X.509 chain is informational only",
            );
        }
        let tls_acceptor = TlsAcceptor::from(tls_server);

        // 10. Spawn accept loops.
        let shutdown_notify = Arc::new(Notify::new());
        let mut bound_addrs: Vec<Option<SocketAddr>> = Vec::with_capacity(config.listeners.len());
        let mut listener_tasks: Vec<JoinHandle<()>> = Vec::new();

        for l in &config.listeners {
            let addr: SocketAddr = match l.addr.parse() {
                Ok(a) => a,
                Err(e) => {
                    return Err(BootError::Bind(format!(
                        "addr {} is not a SocketAddr: {e}",
                        l.addr
                    )));
                }
            };
            // MVP: accept loops only for TCP+TlsExporter. WS and PlainLoopback
            // are recognised but skipped (with a warning) so an operator config
            // listing them doesn't fail-closed.
            if !(l.kind == ListenerKind::Tcp && l.profile == ProfileKind::TlsExporter) {
                tracing::warn!(
                    addr = %addr,
                    kind = ?l.kind,
                    profile = ?l.profile,
                    "listener skipped (MVP boot path supports tcp+tls_exporter only)",
                );
                bound_addrs.push(None);
                continue;
            }

            let listener =
                bind_validated(addr, ListenerProfile::TlsExporter)
                    .await
                    .map_err(|e| BootError::Bind(format!("{addr}: {e}")))?;
            let local_addr = listener
                .local_addr()
                .map_err(|e| BootError::Bind(format!("local_addr: {e}")))?;
            tracing::info!(local_addr = %local_addr, "listener bound (tcp+tls_exporter)");
            bound_addrs.push(Some(local_addr));

            let kdf_default_for_listener = kdf_for_bootstrap;
            let listener_kdf_override = l
                .kdf_override
                .as_ref()
                .map(kdf_from_config);

            // Build the per-listener ConnectionContext.
            let identity_keypair = Ed25519Keypair::from_seed(&identity_seed);
            let ctx = ConnectionContext::new(
                identity.clone(),
                identity_keypair,
                secrets.clone(),
                kdf_default_for_listener,
                session_store.clone(),
                user_dir.clone(),
                lockout.clone(),
                rate_limit.clone(),
                argon2_sem.clone(),
                audit_writer.clone(),
                resume_config.clone(),
                handler.clone(),
                BindingMode::TlsExporter,
                TransportKind::Tcp,
                listener_kdf_override,
            );

            let acceptor = tls_acceptor.clone();
            let notify = shutdown_notify.clone();
            let handle = tokio::spawn(accept_loop_tls_exporter(
                listener,
                acceptor,
                ctx,
                notify,
            ));
            listener_tasks.push(handle);
        }

        Ok(ServerHandle {
            bound_addrs,
            listener_tasks,
            scheduler,
            audit_appender,
            shutdown_notify,
        })
    }
}

/// Per-listener accept loop for the TCP + TLS-exporter profile.
async fn accept_loop_tls_exporter(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ConnectionContext>,
    shutdown: Arc<Notify>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                tracing::debug!("accept_loop: shutdown notified, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "accept failed; sleeping briefly");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let tls = match acceptor.accept(tcp).await {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::debug!(?peer_addr, ?e, "tls handshake failed");
                            return;
                        }
                    };
                    handle_connection(ctx, peer_addr, tls).await;
                });
            }
        }
    }
}

fn kdf_from_config(c: &crate::config::KdfConfig) -> KdfParams {
    KdfParams {
        memory_kb: c.memory_kb,
        time: c.time,
        parallelism: c.parallelism,
        argon2_version: c.argon2_version,
    }
}

fn log_bootstrap_outcome(name: &str, outcome: &BootstrapOutcome) {
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
        BootstrapOutcome::Created { token: Some(tok), token_path } => {
            // SECURITY: print the token at WARN so operators see it, but do
            // NOT log it on every restart. Once the operator changes the
            // password, the file should be deleted so this branch never
            // fires again (Created only happens once).
            tracing::warn!(
                user = %name,
                token = %tok,
                token_path = ?token_path,
                "bootstrap: superuser created with one-time token — \
                 LOG IN AND CHANGE PASSWORD, THEN DELETE THE TOKEN FILE",
            );
        }
    }
    // Suppress dead-code in non-tracing builds (defensive).
    let _ = ServerSecrets {
        server_secret: [0u8; 32],
        lockout_secret: [0u8; 32],
    };
}
