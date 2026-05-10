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

use shamir_db::db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::db::shamir_db::SystemStoreConfig;
use shamir_db::db::ShamirDb;

use shamir_transport_tcp::listener::{
    bind_validated as bind_tcp, ListenerProfile as TcpListenerProfile,
};
use shamir_transport_tcp::tls::extract_tls_exporter;
use shamir_transport_ws::browser::BrowserOriginPolicy;
use shamir_transport_ws::listener::{bind_validated as bind_ws, WsListenerProfile};
use shamir_transport_ws::server::{accept_browser_ws, accept_native_ws};

use crate::framer::{TcpFramer, WsFramer};

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
use crate::tables_registry::TablesRegistry;
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
    #[error("tables_registry: {0}")]
    TablesRegistry(#[from] crate::tables_registry::RegistryError),
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

        // 5. ShamirDb — durable system store + a pre-configured `default.main`
        //    repo backed by redb. Layout under `data_dir`:
        //
        //      shamir_db_meta.redb         — system store (db/repo/setting metadata)
        //      shamir_db_default_main.redb — the `default.main` repo data
        //
        //    Wire-side admin ops can still create additional dbs/repos, but
        //    those are in-memory (`ShamirAdminExecutor` hardcodes engine =
        //    "in_memory"; a future patch can extend it to accept "redb" with
        //    auto-derived paths). For v1 this means the `default.main` repo
        //    is the durable target for application data.
        let meta_path = config.data_dir.join("shamir_db_meta.redb");
        let default_main_path = config.data_dir.join("shamir_db_default_main.redb");
        let shamir = Arc::new(
            ShamirDb::init(SystemStoreConfig::Redb(meta_path))
                .await
                .map_err(|e| BootError::ShamirDbInit(e.to_string()))?,
        );

        // Idempotent: on the first boot the database+repo are created; on
        // every subsequent boot `ShamirDb::init` already loaded them from
        // the system store, so we skip and just use them.
        if !shamir.has_db("default") {
            let _ = shamir.create_db("default").await;
        }
        let default_db = shamir
            .get_db("default")
            .expect("default db must exist after create_db");
        if !default_db.has_repo("main") {
            let factory = BoxRepoFactory::redb(&default_main_path);
            shamir
                .add_repo("default", RepoConfig::new("main", factory))
                .await
                .map_err(|e| BootError::ShamirDbInit(format!("add_repo default.main: {e}")))?;
            tracing::info!(path = ?default_main_path, "created durable default.main repo");
        }

        // Replay the wire-tables registry: re-register every table that a
        // wire client created in a previous boot so its data file is
        // re-attached to the in-memory `RepoInstance`. Without this step
        // the redb file on disk still exists but the running server has no
        // table config pointing at it, so reads return "table not found".
        let tables_registry = Arc::new(TablesRegistry::open(&config.data_dir)?);
        {
            let snap = tables_registry.snapshot();
            for (db_name, repo_name, table_name) in snap.iter_entries() {
                if let Some(db) = shamir.get_db(db_name) {
                    if !db.has_table(repo_name, table_name) {
                        if let Err(e) = db.create_table(repo_name, table_name) {
                            tracing::warn!(
                                db = db_name, repo = repo_name, table = table_name,
                                ?e, "tables_registry replay: create_table failed"
                            );
                        }
                    }
                }
            }
        }
        let handler: Arc<dyn RequestHandler> = Arc::new(ShamirDbHandler::with_admin(
            shamir.clone(),
            AdminGlue {
                user_dir: user_dir.clone(),
                kdf: kdf_for_bootstrap,
                tables_registry: Some(tables_registry.clone()),
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

            // Resolve the right (transport, binding_mode) tuple for this
            // listener. Plain (binding_mode = 0x00) is still routed through
            // the TLS code path for now — a follow-up will add a true
            // plain-tcp accept loop for loopback debugging.
            let (transport_kind, binding_mode, accept_path) = match (l.kind, l.profile) {
                (ListenerKind::Tcp, ProfileKind::TlsExporter) => (
                    TransportKind::Tcp,
                    BindingMode::TlsExporter,
                    AcceptPath::TcpTlsExporter,
                ),
                (ListenerKind::Ws, ProfileKind::TlsExporter) => (
                    TransportKind::WebSocket,
                    BindingMode::TlsExporter,
                    AcceptPath::WsNative,
                ),
                (ListenerKind::Ws, ProfileKind::TlsNoExport) => {
                    let policy = browser_origin_policy_from(&l.browser_origin_allowlist)?;
                    (
                        TransportKind::WebSocket,
                        BindingMode::TlsNoExport,
                        AcceptPath::WsBrowser(policy),
                    )
                }
                (kind, profile) => {
                    tracing::warn!(
                        addr = %addr,
                        ?kind,
                        ?profile,
                        "listener skipped (unsupported MVP combination)",
                    );
                    bound_addrs.push(None);
                    continue;
                }
            };

            let local_addr = match accept_path {
                AcceptPath::TcpTlsExporter => {
                    let listener = bind_tcp(addr, TcpListenerProfile::TlsExporter)
                        .await
                        .map_err(|e| BootError::Bind(format!("tcp {addr}: {e}")))?;
                    let local_addr = listener
                        .local_addr()
                        .map_err(|e| BootError::Bind(format!("local_addr: {e}")))?;
                    tracing::info!(local_addr = %local_addr, kind = ?l.kind, profile = ?l.profile, "listener bound");
                    let ctx = build_ctx(
                        &identity,
                        identity_seed,
                        &secrets,
                        kdf_for_bootstrap,
                        &session_store,
                        &user_dir,
                        lockout.clone(),
                        rate_limit.clone(),
                        &argon2_sem,
                        &audit_writer,
                        &resume_config,
                        &handler,
                        binding_mode,
                        transport_kind,
                        l.kdf_override.as_ref().map(kdf_from_config),
                    );
                    let acceptor = tls_acceptor.clone();
                    let notify = shutdown_notify.clone();
                    listener_tasks.push(tokio::spawn(accept_loop_tcp(listener, acceptor, ctx, notify)));
                    local_addr
                }
                AcceptPath::WsNative => {
                    let listener = bind_ws(addr, WsListenerProfile::Wss)
                        .await
                        .map_err(|e| BootError::Bind(format!("ws {addr}: {e}")))?;
                    let local_addr = listener
                        .local_addr()
                        .map_err(|e| BootError::Bind(format!("local_addr: {e}")))?;
                    tracing::info!(local_addr = %local_addr, kind = ?l.kind, profile = ?l.profile, "listener bound");
                    let ctx = build_ctx(
                        &identity,
                        identity_seed,
                        &secrets,
                        kdf_for_bootstrap,
                        &session_store,
                        &user_dir,
                        lockout.clone(),
                        rate_limit.clone(),
                        &argon2_sem,
                        &audit_writer,
                        &resume_config,
                        &handler,
                        binding_mode,
                        transport_kind,
                        l.kdf_override.as_ref().map(kdf_from_config),
                    );
                    let acceptor = tls_acceptor.clone();
                    let notify = shutdown_notify.clone();
                    listener_tasks.push(tokio::spawn(accept_loop_ws_native(
                        listener, acceptor, ctx, notify,
                    )));
                    local_addr
                }
                AcceptPath::WsBrowser(policy) => {
                    let listener = bind_ws(addr, WsListenerProfile::WssBrowser)
                        .await
                        .map_err(|e| BootError::Bind(format!("ws {addr}: {e}")))?;
                    let local_addr = listener
                        .local_addr()
                        .map_err(|e| BootError::Bind(format!("local_addr: {e}")))?;
                    tracing::info!(local_addr = %local_addr, kind = ?l.kind, profile = ?l.profile, "listener bound");
                    let ctx = build_ctx(
                        &identity,
                        identity_seed,
                        &secrets,
                        kdf_for_bootstrap,
                        &session_store,
                        &user_dir,
                        lockout.clone(),
                        rate_limit.clone(),
                        &argon2_sem,
                        &audit_writer,
                        &resume_config,
                        &handler,
                        binding_mode,
                        transport_kind,
                        l.kdf_override.as_ref().map(kdf_from_config),
                    );
                    let acceptor = tls_acceptor.clone();
                    let notify = shutdown_notify.clone();
                    listener_tasks.push(tokio::spawn(accept_loop_ws_browser(
                        listener, acceptor, ctx, notify, policy,
                    )));
                    local_addr
                }
            };
            bound_addrs.push(Some(local_addr));
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

/// Per-listener loop dispatch — picked at config-validation time, used at
/// boot to decide which `accept_loop_*` to spawn.
enum AcceptPath {
    /// `kind=tcp + profile=tls_exporter`: native binding_mode 0x01.
    TcpTlsExporter,
    /// `kind=ws + profile=tls_exporter`: native WSS, binding_mode 0x01.
    WsNative,
    /// `kind=ws + profile=tls_no_export`: browser WSS, binding_mode 0x02
    /// with mandatory Origin allowlist.
    WsBrowser(BrowserOriginPolicy),
}

/// Build a `BrowserOriginPolicy` from the listener's `browser_origin_allowlist`.
/// Empty allowlist is rejected at config validation, so by the time we get
/// here we always have at least one origin.
fn browser_origin_policy_from(allowlist: &[String]) -> Result<BrowserOriginPolicy, BootError> {
    if allowlist.is_empty() {
        return Err(BootError::Bind(
            "browser_origin_allowlist must not be empty for tls_no_export ws listeners".into(),
        ));
    }
    Ok(BrowserOriginPolicy::allow(allowlist.iter().cloned()))
}

/// Build a per-listener `ConnectionContext`. Each listener gets its own
/// `Ed25519Keypair` reconstructed from the same identity seed — this is a
/// workaround for shamir-connect's `verify_proof` API requiring
/// `&Ed25519Keypair` while `ServerIdentityState` doesn't expose the keypair.
#[allow(clippy::too_many_arguments)]
fn build_ctx(
    identity: &Arc<shamir_connect::server::rotation::ServerIdentityState>,
    identity_seed: [u8; 32],
    secrets: &Arc<ServerSecrets>,
    kdf: KdfParams,
    session_store: &Arc<SessionStore>,
    user_dir: &Arc<RedbUserDirectory>,
    lockout: Arc<dyn shamir_connect::server::lockout::LockoutStore>,
    rate_limit: Arc<dyn shamir_connect::server::rate_limit::RateLimiter>,
    argon2_sem: &Arc<Argon2Semaphore>,
    audit_writer: &Arc<AuditChainWriter>,
    resume_config: &Arc<ResumeConfig>,
    handler: &Arc<dyn shamir_connect::server::dispatch::RequestHandler>,
    binding_mode: BindingMode,
    transport_kind: TransportKind,
    kdf_override: Option<KdfParams>,
) -> Arc<ConnectionContext> {
    let identity_keypair = Ed25519Keypair::from_seed(&identity_seed);
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
        handler.clone(),
        binding_mode,
        transport_kind,
        kdf_override,
    )
}

/// TCP+TLS accept loop. The accept_loop itself never blocks: every step
/// is async, and per-connection work is `tokio::spawn`'d so multiple
/// in-flight TLS handshakes / Argon2id verifies don't serialise on the
/// listener.
async fn accept_loop_tcp(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ConnectionContext>,
    shutdown: Arc<Notify>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                tracing::debug!("accept_loop_tcp: shutdown notified, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "tcp accept failed; sleeping briefly");
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
                    let exporter = extract_tls_exporter(&tls).unwrap_or([0u8; 32]);
                    let framer = TcpFramer::new(tls);
                    handle_connection(ctx, peer_addr, framer, exporter).await;
                });
            }
        }
    }
}

/// Native WSS accept loop — `tcp -> tls -> ws upgrade -> handle_connection`
/// with the TLS exporter extracted before the WS upgrade consumes the
/// stream. binding_mode = TlsExporter.
async fn accept_loop_ws_native(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ConnectionContext>,
    shutdown: Arc<Notify>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                tracing::debug!("accept_loop_ws_native: shutdown notified, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "ws accept failed; sleeping briefly");
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
                            tracing::debug!(?peer_addr, ?e, "tls handshake failed (ws native)");
                            return;
                        }
                    };
                    // CRITICAL: extract exporter BEFORE the WS upgrade
                    // consumes `tls`. After upgrade the TLS state is owned
                    // by the WebSocketStream and not directly accessible.
                    let exporter = extract_tls_exporter(&tls).unwrap_or([0u8; 32]);
                    let ws = match accept_native_ws(tls).await {
                        Ok(ws) => ws,
                        Err(e) => {
                            tracing::debug!(?peer_addr, ?e, "ws native upgrade failed");
                            return;
                        }
                    };
                    let framer = WsFramer::new(ws);
                    handle_connection(ctx, peer_addr, framer, exporter).await;
                });
            }
        }
    }
}

/// Browser WSS accept loop — origin-allowlist enforced inside
/// `accept_browser_ws`. binding_mode = TlsNoExport (exporter = zeros per
/// spec §6.4 because the JS environment can't access it).
async fn accept_loop_ws_browser(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ConnectionContext>,
    shutdown: Arc<Notify>,
    policy: BrowserOriginPolicy,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                tracing::debug!("accept_loop_ws_browser: shutdown notified, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "ws-browser accept failed; sleeping briefly");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                let policy = policy.clone();
                tokio::spawn(async move {
                    let tls = match acceptor.accept(tcp).await {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::debug!(?peer_addr, ?e, "tls handshake failed (ws browser)");
                            return;
                        }
                    };
                    // Browser binding mode: exporter MUST be zeros per spec.
                    let exporter = [0u8; 32];
                    let ws = match accept_browser_ws(tls, &policy).await {
                        Ok(ws) => ws,
                        Err(e) => {
                            tracing::debug!(?peer_addr, ?e, "ws browser upgrade failed");
                            return;
                        }
                    };
                    let framer = WsFramer::new(ws);
                    handle_connection(ctx, peer_addr, framer, exporter).await;
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
