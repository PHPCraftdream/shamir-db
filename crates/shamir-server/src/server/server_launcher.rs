//! Boot orchestration — [`ServerLauncher`] owns the [`Config`] + bootstrap
//! policy and produces a [`ServerHandle`] when launched.
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
use shamir_connect::server::lockout::{InMemoryLockoutStore, LockoutSnapshotSink};
use shamir_connect::server::rate_limit::{InMemoryRateLimiter, RateLimitSnapshotSink};
use shamir_connect::server::resume::{InMemoryConsumedCounters, ResumeConfig};
use shamir_connect::server::session::SessionStore;

use shamir_tunables::runtime::RuntimeTunables;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;

use shamir_transport_tcp::listener::{
    bind_validated as bind_tcp, ListenerProfile as TcpListenerProfile,
};
use shamir_transport_tcp::tls::extract_tls_exporter;
use shamir_transport_ws::browser::BrowserOriginPolicy;
use shamir_transport_ws::listener::{bind_validated as bind_ws, WsListenerProfile};
use shamir_transport_ws::server::{accept_browser_ws, accept_native_ws};

use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::audit_appender::RedbAuditAppender;
use crate::bootstrap::{
    ensure_superuser, BootstrapOutcome, BootstrapPolicy, DEFAULT_BOOTSTRAP_NAME,
};
use crate::config::{Config, ListenerKind, ProfileKind};
use crate::conn_limiter::ConnLimiter;
use crate::connection::{handle_connection, ConnectionContext};
use crate::db_handler::{AdminGlue, QueryLimitsCap, ShamirDbHandler, SlowQueryConfig, TxLimitsCap};
use crate::framer::{TcpFramer, WsFramer};
use crate::scheduler::{Scheduler, SchedulerConfig, SchedulerInputs};
use crate::server::boot_error::BootError;
use crate::server::bootstrap_mode::BootstrapMode;
use crate::server::meta_sinks::{spawn_meta_snapshot_task, MetaLockoutSink, MetaRateLimitSink};
use crate::server::server_handle::ServerHandle;
use crate::server_meta::ServerMetaStore;
use crate::tables_registry::TablesRegistry;
use crate::tls::{load_or_generate, subject_alts_from_addrs, LoadedTls};
use crate::user_directory::RedbUserDirectory;

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

        // Root shutdown token — the single source of truth for "the server
        // is stopping". Every long-lived task (accept loops, meta-snapshot,
        // tx-reaper) holds a clone; `ServerHandle::shutdown` calls `.cancel()`
        // exactly once and the signal cascades to all of them. The tree of
        // tasks maps onto one cancellation: shutdown is a LEVEL (a state),
        // and CancellationToken is level-triggered (persistent) — unlike
        // `Notify::notify_waiters`, which is edge-triggered and lossy across
        // the `select!` subscribe-window (a notify that lands while a task is
        // executing its tick-branch body, not parked in `select!`, is
        // silently dropped — the cold-start hang we chased through the accept
        // loops). One concept, race-free by construction.
        let shutdown_token = tokio_util::sync::CancellationToken::new();

        // 1. Durable stores.
        std::fs::create_dir_all(&config.data_dir)?;

        // Single-instance guard: acquire an advisory OS file lock on
        // `<data_dir>/.shamir.lock` BEFORE opening any redb store. The
        // lock is held for the entire process lifetime via the `File`
        // stored in `ServerHandle._data_dir_lock` — drop releases it
        // (and the kernel releases on crash). This prevents two servers
        // from opening the same data_dir and corrupting/contending the
        // redb stores.
        let lock_path = config.data_dir.join(".shamir.lock");
        let data_dir_lock = {
            use fs4::fs_std::FileExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)?;
            file.try_lock_exclusive()
                .map_err(|_| BootError::AlreadyRunning(config.data_dir.clone()))?;
            // Best-effort: write the current pid for operator diagnostics.
            // The lock, not the contents, is authoritative.
            let _ = (|| -> std::io::Result<()> {
                use std::io::Write;
                let mut f = &file;
                f.write_all(std::process::id().to_string().as_bytes())?;
                f.flush()
            })();
            file
        };

        let meta = Arc::new(
            ServerMetaStore::open_or_init(config.data_dir.join("server_meta.redb"))
                .map_err(|e| BootError::ServerMeta(e.to_string()))?,
        );

        let user_dir = Arc::new(
            RedbUserDirectory::open(config.data_dir.join("users.redb"))
                .map_err(|e| BootError::UserDirectory(e.to_string()))?,
        );
        let counters = Arc::new(
            RedbConsumedCounters::open(config.data_dir.join("counters.redb"))
                .map_err(|e| BootError::Counters(e.to_string()))?,
        );

        let audit_max_bytes = if config.audit.max_file_size_mb == 0 {
            None
        } else {
            Some(config.audit.max_file_size_mb.saturating_mul(1024 * 1024))
        };
        let audit_appender = RedbAuditAppender::open_batched_with_rotation(
            &config.data_dir,
            std::time::Duration::from_secs(5),
            audit_max_bytes,
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
        //
        // Lockout state is rehydrated from `server_meta.redb` and a
        // background task (`LOCKOUT_SNAPSHOT_INTERVAL` below) persists it
        // every 60s so brute-force bookkeeping survives server restarts
        // — otherwise an attacker who knows the restart cadence resets
        // all per-pair failure counts and re-bursts the auth path with
        // no lockout penalty. See `shamir_connect::server::lockout` for
        // the trade-off rationale (≤60s loss window vs fsync-per-failure).
        let lockout_sink: Arc<dyn LockoutSnapshotSink> =
            Arc::new(MetaLockoutSink::new(meta.clone()));
        let lockout = Arc::new(InMemoryLockoutStore::with_snapshot_sink(lockout_sink));
        let now_ns = UnixNanos::now().as_u64();
        // Rate-limit buckets are persisted by the SAME periodic task as
        // lockout (one 60s tick writes both). On boot the depleted-bucket
        // state is rehydrated conservatively: token levels survive but each
        // bucket's refill clock is re-anchored to `now_ns`, so the downtime
        // grants no free refill (the secure direction). The §8.6 warmup
        // window (re-armed from `now_ns`) layers extra throttling on top
        // for the first 60s. See `shamir_connect::server::rate_limit`.
        let rate_limit_sink: Arc<dyn RateLimitSnapshotSink> =
            Arc::new(MetaRateLimitSink::new(meta.clone()));
        let rate_limit = Arc::new(InMemoryRateLimiter::with_snapshot_sink_and_rate(
            rate_limit_sink,
            now_ns,
            config.security.auth_init_rate_per_second,
        ));
        let argon2_sem = Arc::new(Argon2Semaphore::with_capacity(config.argon2_concurrent_max));
        let session_store = Arc::new(SessionStore::new());

        // Spawn the periodic meta-snapshot task. One 60s tick persists
        // BOTH the lockout store and the rate-limiter buckets. Errors are
        // logged and ignored — losing a snapshot is not fatal, the next
        // tick will retry, and a hard crash between writes loses at most
        // one interval of new state. The returned handle is stored on
        // `ServerHandle` so `shutdown()` can join it (otherwise it would
        // hold the redb file lock past shutdown and block any
        // same-data-dir restart).
        let meta_snapshot_task = Some(spawn_meta_snapshot_task(
            lockout.clone(),
            rate_limit.clone(),
            shutdown_token.clone(),
        ));

        // 4. Audit chain — load from checkpoint if present.
        //
        // CRITICAL: the writer (used by every connection task to emit
        // audit events) and the scheduler (which periodically persists
        // the truncation-defence checkpoint) MUST share a single chain
        // state. Constructing two independent `AuditChain` instances
        // here produces a split-brain — appends advance one chain while
        // the checkpoint snapshots the other (empty) one, so audit.log
        // restarts at seq=1 on every reboot. See
        // `audit_writer_and_checkpoint_share_chain_state` for the
        // regression test.
        let ack = meta
            .audit_chain_key()
            .map_err(|e| BootError::ServerMeta(e.to_string()))?;
        let audit_chain = match meta.audit_checkpoint() {
            Some((seq, hmac)) => Arc::new(AuditChain::from_checkpoint(ack, seq, hmac)),
            None => Arc::new(AuditChain::new(ack)),
        };
        let audit_writer = Arc::new(AuditChainWriter::new_with_shared(
            audit_chain.clone(),
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
                                db = db_name,
                                repo = repo_name,
                                table = table_name,
                                ?e,
                                "tables_registry replay: create_table failed"
                            );
                        }
                    }
                }
            }
        }
        let handler_concrete: Arc<ShamirDbHandler> = Arc::new(
            ShamirDbHandler::with_admin(
                shamir.clone(),
                AdminGlue {
                    user_dir: user_dir.clone(),
                    kdf: kdf_for_bootstrap,
                    tables_registry: Some(tables_registry.clone()),
                },
            )
            .with_slow_query(SlowQueryConfig::from_ms(
                config.logging.slow_query_threshold_ms,
            ))
            .with_query_limits(QueryLimitsCap {
                max_result_size_bytes: config.security.query_limits.max_result_size_bytes,
                max_execution_time_secs: config.security.query_limits.max_execution_time_secs,
                max_queries_per_batch: config.security.query_limits.max_queries_per_batch,
            })
            .with_tx_limits(TxLimitsCap {
                max_tx_bytes: config.security.tx.max_tx_bytes,
            }),
        );
        let tx_registry_for_reaper = handler_concrete.tx_registry();
        let handler: Arc<dyn RequestHandler> = handler_concrete;

        // Phase B Stage 6 — background reaper for interactive txs that
        // outlive their idle TTL or absolute deadline. Shares the root
        // shutdown token: `ServerHandle::shutdown` cancels it once and this
        // task observes the cancel on its next `select!` poll.
        let interactive_tx_reaper = Some(crate::tx_registry::spawn_reaper_task(
            tx_registry_for_reaper,
            crate::tx_registry::DEFAULT_INTERACTIVE_TX_IDLE_TTL,
            crate::tx_registry::DEFAULT_REAPER_INTERVAL,
            shutdown_token.clone(),
        ));

        // 6. ResumeConfig.
        let (current_key, previous_key) = meta
            .ticket_keys()
            .map_err(|e| BootError::ServerMeta(e.to_string()))?;
        let resume_config = Arc::new(ResumeConfig::new(
            current_key,
            previous_key,
            true,  // allow_browser_ticket_upgrade
            false, // disable_plain_ticket_upgrade
        ));

        // 7. Identity material.
        let identity = Arc::new(
            meta.identity_state()
                .map_err(|e| BootError::ServerMeta(e.to_string()))?,
        );
        let identity_seed = meta
            .current_identity_seed()
            .map_err(|e| BootError::ServerMeta(e.to_string()))?;
        let secrets = Arc::new(
            meta.server_secrets()
                .map_err(|e| BootError::ServerMeta(e.to_string()))?,
        );

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
        let LoadedTls {
            server_config: tls_server,
            generated,
        } = load_or_generate(
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

        // 10. Spawn accept loops. They share the root `shutdown_token`
        //     created at the top of `launch()` — `shutdown()` cancels it
        //     once and every accept loop observes the cancel on its next
        //     `select!` poll.
        let mut bound_addrs: Vec<Option<SocketAddr>> = Vec::with_capacity(config.listeners.len());
        let mut listener_tasks: Vec<JoinHandle<()>> = Vec::new();

        // Connection-level security knobs from config (slow-loris timeout, etc.)
        let auth_init_timeout =
            Duration::from_millis(config.security.connection.auth_init_timeout_ms);
        // Global cap on simultaneously-active connections — shared across
        // every listener.
        let conn_limiter = ConnLimiter::new(config.security.connection.max_active_connections);

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
                        auth_init_timeout,
                    );
                    let acceptor = tls_acceptor.clone();
                    let token = shutdown_token.clone();

                    let limiter = conn_limiter.clone();
                    listener_tasks.push(tokio::spawn(accept_loop_tcp(
                        listener, acceptor, ctx, token, limiter,
                    )));
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
                        auth_init_timeout,
                    );
                    let acceptor = tls_acceptor.clone();
                    let token = shutdown_token.clone();

                    let limiter = conn_limiter.clone();
                    listener_tasks.push(tokio::spawn(accept_loop_ws_native(
                        listener, acceptor, ctx, token, limiter,
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
                        auth_init_timeout,
                    );
                    let acceptor = tls_acceptor.clone();
                    let token = shutdown_token.clone();

                    let limiter = conn_limiter.clone();
                    listener_tasks.push(tokio::spawn(accept_loop_ws_browser(
                        listener, acceptor, ctx, token, limiter, policy,
                    )));
                    local_addr
                }
            };
            bound_addrs.push(Some(local_addr));
        }

        // 11. Optional observability HTTP server. Spawned LAST so its
        //     `/readyz` flips to 200 only after every listener is bound.
        let observability = if config.observability.addr.is_empty() {
            None
        } else {
            let addr: std::net::SocketAddr = config.observability.addr.parse().map_err(|e| {
                BootError::Bind(format!(
                    "observability.addr {} invalid: {e}",
                    config.observability.addr
                ))
            })?;
            let state = crate::observability::ObservabilityState::new();
            // Record what the data-path bound to — `/info` surfaces this.
            state.set_bound_addrs(bound_addrs.iter().filter_map(|a| *a).collect());
            // The Prometheus recorder is global per-process. In integration
            // tests where multiple servers spawn back-to-back the second
            // launcher would error on `set_global_recorder`. We swallow
            // that specific failure (recorder already installed = OK,
            // we use the existing one) and continue without an exporter
            // handle in that case.
            // M-tier audit M5: pass `allow_public_metrics = false`. A
            // non-loopback `addr` is rejected up-front. Operators that
            // need a public scrape endpoint can promote this to a
            // config flag in a follow-up.
            let handle =
                match crate::observability::spawn(addr, state.clone(), true, None, false).await {
                    Ok(h) => h,
                    Err(crate::observability::ObservabilityError::RecorderInstall(_)) => {
                        // Recorder already installed (typical in test process):
                        // re-spawn without trying to install again.
                        crate::observability::spawn(addr, state.clone(), false, None, false)
                            .await
                            .map_err(|e| BootError::Bind(format!("observability: {e}")))?
                    }
                    Err(e) => return Err(BootError::Bind(format!("observability: {e}"))),
                };
            handle.state.mark_ready();
            Some(handle)
        };

        Ok(ServerHandle {
            bound_addrs,
            listener_tasks,
            scheduler,
            audit_appender,
            shutdown_token,

            observability,
            meta_snapshot_task,
            interactive_tx_reaper,
            shamir,
            _data_dir_lock: data_dir_lock,
            tunables: Arc::new(RuntimeTunables::new()),
        })
    }
}

// ---------------------------------------------------------------------------
// Private helpers — only used within ServerLauncher::launch
// ---------------------------------------------------------------------------

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
    auth_init_timeout: Duration,
) -> Arc<ConnectionContext> {
    let identity_keypair = Ed25519Keypair::from_seed(&identity_seed);
    let max_in_flight = shamir_tunables::instance_defaults::CONN_MAX_IN_FLIGHT;
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
        max_in_flight,
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
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_tcp: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "tcp accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                // Reserve a slot BEFORE the TLS handshake so a saturated
                // server doesn't waste CPU on TLS for connections we're
                // about to close. Drop closes the TCP socket immediately
                // (the `tcp` binding goes out of scope) — kernel sends RST.
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ?peer_addr,
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached, refusing",
                        );
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let _guard = guard;  // keep alive for the lifetime of this task
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
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_ws_native: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "ws accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ?peer_addr,
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached (ws native), refusing",
                        );
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let _guard = guard;
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
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
    policy: BrowserOriginPolicy,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_ws_browser: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "ws-browser accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ?peer_addr,
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached (ws browser), refusing",
                        );
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                let policy = policy.clone();
                tokio::spawn(async move {
                    let _guard = guard;
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
