//! ShamirDB production server binary.
//!
//! Boot sequence:
//! 1. Parse `--config <path.ktav>` CLI flag.
//! 2. Load + validate `Config` (ktav).
//! 3. Open `ServerMetaStore` (auto-init on first start).
//! 4. Open `RedbUserDirectory` + `RedbConsumedCounters` + start
//!    `RedbAuditAppender` (batched mode).
//! 5. Construct in-memory stores: `SessionStore`, `InMemoryLockoutStore`,
//!    `InMemoryRateLimiter`, `Argon2Semaphore`, `AuditChain` from
//!    checkpoint.
//! 6. Spawn `Scheduler` (background gc / checkpoint / identity finalize).
//! 7. For each listener in config: bind via `bind_validated`, spawn an
//!    accept loop that calls into `connection::handle_connection`.
//! 8. Wait for SIGTERM; on shutdown drain audit + flush durable state.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use shamir_connect::server::audit_chain::{AuditChain, AuditChainWriter};
use shamir_connect::server::durable_counters::RedbConsumedCounters;
use shamir_connect::server::lockout::InMemoryLockoutStore;
use shamir_connect::server::rate_limit::InMemoryRateLimiter;
use shamir_connect::server::resume::ResumeConfig;
use shamir_connect::server::session::SessionStore;
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_db::db::ShamirDb;

use shamir_server::audit_appender::RedbAuditAppender;
use shamir_server::config::Config;
use shamir_server::db_handler::ShamirDbHandler;
use shamir_server::scheduler::{Scheduler, SchedulerConfig, SchedulerInputs};
use shamir_server::server_meta::ServerMetaStore;
use shamir_server::user_directory::RedbUserDirectory;

#[derive(Parser, Debug)]
#[command(name = "shamir-server", version, about = "ShamirDB production server")]
struct Cli {
    /// Path to the .ktav config file.
    #[arg(short, long)]
    config: PathBuf,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 1. Load + validate config.
    let config = Config::from_file(&cli.config)?;
    config.validate()?;

    // 2. Init tracing.
    let filter = tracing_subscriber::EnvFilter::try_new(&config.logging.level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    tracing::info!(data_dir = ?config.data_dir, "shamir-server boot");

    // 3. Open durable stores.
    std::fs::create_dir_all(&config.data_dir)?;
    let meta = ServerMetaStore::open_or_init(config.data_dir.join("server_meta.redb"))
        .map_err(|e| anyhow::anyhow!("server_meta: {e}"))?;

    let user_dir = Arc::new(
        RedbUserDirectory::open(config.data_dir.join("users.redb"))
            .map_err(|e| anyhow::anyhow!("user_directory: {e}"))?,
    );
    let counters = Arc::new(
        RedbConsumedCounters::open(config.data_dir.join("counters.redb"))
            .map_err(|e| anyhow::anyhow!("counters: {e}"))?,
    );

    let audit_appender = RedbAuditAppender::open_batched(
        &config.data_dir,
        std::time::Duration::from_secs(5),
    )
    .map_err(|e| anyhow::anyhow!("audit_appender: {e}"))?;

    // 4. In-memory stores.
    let lockout = Arc::new(InMemoryLockoutStore::new());
    let now_ns = shamir_connect::common::time::UnixNanos::now().as_u64();
    let rate_limit = Arc::new(InMemoryRateLimiter::new(now_ns));
    let argon2_sem = Arc::new(Argon2Semaphore::with_capacity(config.argon2_concurrent_max));
    let session_store = Arc::new(SessionStore::new());

    // 5. Audit chain — load from checkpoint if present.
    let audit_chain = match meta.audit_checkpoint() {
        Some((seq, hmac)) => Arc::new(AuditChain::from_checkpoint(meta.audit_chain_key(), seq, hmac)),
        None => Arc::new(AuditChain::new(meta.audit_chain_key())),
    };
    let audit_writer = Arc::new(AuditChainWriter::new(
        AuditChain::new(meta.audit_chain_key()), // separate chain instance for the writer side
        audit_appender.clone() as Arc<dyn shamir_connect::server::audit_chain::AuditAppender>,
    ));

    // 6. ShamirDb (in-memory for v1; real persistence is a follow-up).
    let shamir = Arc::new(ShamirDb::init_memory().await?);
    let handler = Arc::new(ShamirDbHandler::new(shamir.clone()));

    // 7. ResumeConfig.
    let (current_key, previous_key) = meta.ticket_keys();
    let resume_config = Arc::new(ResumeConfig::new(
        current_key,
        previous_key,
        true,  // allow_browser_ticket_upgrade
        false, // disable_plain_ticket_upgrade
    ));

    // 8. Identity state (loaded from meta).
    let identity = Arc::new(meta.identity_state());

    // 9. Scheduler.
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

    // 10. Listeners — for v1 we log the config and wait for SIGTERM.
    // Full per-listener accept loops are wired via connection::handle_connection
    // (see connection.rs); the integration with TLS cert load + per-profile
    // dispatch is intentionally minimal here so the binary at least boots.
    tracing::info!(
        listeners = config.listeners.len(),
        "shamir-server ready (listeners not yet bound — see PRODUCTION_SERVER_PLAN.md)",
    );

    // Suppress unused-warnings for the wired-but-not-yet-spawned pieces.
    let _ = (
        user_dir,
        argon2_sem,
        resume_config,
        handler,
        audit_writer,
        meta,
    );

    // SIGTERM-aware wait.
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");

    audit_appender.shutdown().await;
    scheduler.shutdown().await;
    Ok(())
}
