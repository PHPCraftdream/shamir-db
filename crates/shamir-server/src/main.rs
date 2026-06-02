//! ShamirDB production server binary.
//!
//! Boot orchestration lives in [`shamir_server::server::ServerLauncher`] —
//! `main` itself is intentionally tiny: parse CLI, install rustls crypto
//! provider, init tracing, hand the [`Config`] to the launcher, then wait
//! for SIGINT/SIGTERM.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use zeroize::Zeroizing;

use shamir_server::backup;
use shamir_server::config::Config;
use shamir_server::server::BootstrapMode;

#[derive(Parser, Debug)]
#[command(name = "shamir-server", version, about = "ShamirDB production server")]
struct Cli {
    /// Path to the `.ktav` config file.
    #[arg(short, long)]
    config: PathBuf,

    /// Optional username for the bootstrap superuser. Defaults to `admin`.
    #[arg(long, value_name = "NAME")]
    bootstrap_user: Option<String>,

    /// Optional password for the bootstrap superuser. If omitted, a random
    /// 32-byte token is generated, written to `data_dir/bootstrap_token.txt`,
    /// AND printed once at WARN level.
    #[arg(long, value_name = "PASSWORD")]
    bootstrap_password: Option<String>,

    /// Skip the bootstrap step entirely. Use only when the operator manages
    /// the user directory out-of-band.
    #[arg(long)]
    skip_bootstrap: bool,

    /// Optional subcommand. Without one (or with `run`) the server runs
    /// normally in the foreground; with `backup` it performs a one-shot
    /// snapshot and exits.
    #[command(subcommand)]
    command: Option<Subcmd>,
}

#[derive(Subcommand, Debug)]
enum Subcmd {
    /// Run in the foreground (default). Ctrl+C / SIGTERM → graceful
    /// shutdown. Omitting a subcommand is equivalent to `run`.
    Run,

    /// Snapshot `data_dir` (from --config) into `<to>/<UTC-timestamp>/`.
    /// **Server should be stopped first** for a fully consistent snapshot.
    /// redb's per-page CRC + atomic-commit design means a copy taken
    /// during a quiescent window is recoverable as the pre-commit state,
    /// but for confidence stop the server.
    Backup {
        /// Destination directory. Created if missing. The actual snapshot
        /// goes into `<to>/YYYYMMDD_HHMMSS/`.
        #[arg(long, value_name = "DIR")]
        to: PathBuf,
    },

    /// Render the Shomer access-control tree (resources + functions +
    /// principals) and exit. Offline by default (opens `data_dir` from
    /// `--config`; the server must be stopped). With `--connect` it
    /// authenticates to a running server as an admin instead.
    AccessTree {
        /// Resource-depth cap: 0=root, 1=db, 2=store, 3=table. Default: full.
        #[arg(long, value_name = "N")]
        depth: Option<u32>,
        /// Restrict the resource tree to a single database.
        #[arg(long, value_name = "DB")]
        db: Option<String>,
        /// Emit raw JSON instead of the rendered ASCII tree.
        #[arg(long)]
        json: bool,
        /// Online mode: connect to a running server at `host:port` and
        /// request the tree over TLS+SCRAM (requires admin credentials).
        #[arg(long, value_name = "ADDR")]
        connect: Option<String>,
        /// SNI hostname for TLS in online mode (matches the server cert).
        #[arg(long, value_name = "NAME", default_value = "localhost")]
        server_name: String,
        /// Username for online mode (must be an admin).
        #[arg(long, value_name = "NAME")]
        user: Option<String>,
        /// Password for online mode; falls back to `$SHAMIR_PASSWORD`.
        #[arg(long, value_name = "PASSWORD")]
        password: Option<String>,
    },
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Install rustls crypto provider (required by tokio-rustls; second call
    // is a harmless no-op).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Load + validate config so we can configure tracing from it.
    let config = Config::from_file(&cli.config)?;
    config.validate()?;

    // Non-blocking logging: log events flow through a lock-free MPSC
    // channel to a background writer thread so emitting never blocks a
    // worker. The guard must live until process exit for a clean flush.
    let _log_guard = shamir_server::logging::init(&config.logging);

    // Subcommand dispatch — `backup` / `access-tree` exit without booting
    // the server.
    match cli.command {
        Some(Subcmd::Backup { to }) => {
            let report = backup::backup(&config.data_dir, &to)?;
            println!(
                "backup ok: {} files, {} bytes → {}",
                report.files_copied,
                report.bytes_copied,
                report.dest_dir.display()
            );
            return Ok(());
        }
        Some(Subcmd::AccessTree {
            depth,
            db,
            json,
            connect,
            server_name,
            user,
            password,
        }) => {
            let args = shamir_server::access_tree::AccessTreeArgs {
                depth,
                db,
                json,
                connect,
                server_name,
                user,
                password,
            };
            shamir_server::access_tree::run(&config, &args).await?;
            return Ok(());
        }
        Some(Subcmd::Run) | None => {}
    }

    tracing::info!(data_dir = ?config.data_dir, "shamir-server boot");

    // Construct bootstrap policy from CLI flags.
    let bootstrap = if cli.skip_bootstrap {
        BootstrapMode::Skip
    } else if let Some(pw) = cli.bootstrap_password {
        BootstrapMode::Password {
            username: cli
                .bootstrap_user
                .unwrap_or_else(|| shamir_server::bootstrap::DEFAULT_BOOTSTRAP_NAME.to_string()),
            password: Zeroizing::new(pw.into_bytes()),
        }
    } else {
        BootstrapMode::RandomToken {
            username: cli.bootstrap_user,
        }
    };

    shamir_server::runtime::serve(
        config,
        bootstrap,
        shamir_server::runtime::foreground_shutdown(),
    )
    .await
}
