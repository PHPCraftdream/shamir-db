//! ShamirDB production server binary.
//!
//! Boot orchestration lives in [`shamir_server::server::ServerLauncher`] —
//! `main` itself is intentionally tiny: parse CLI, install rustls crypto
//! provider, init tracing, hand the [`Config`] to the launcher, then wait
//! for SIGINT/SIGTERM.

use std::path::PathBuf;

use clap::Parser;
use zeroize::Zeroizing;

use shamir_server::config::Config;
use shamir_server::server::{BootstrapMode, ServerLauncher};

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

    let filter = tracing_subscriber::EnvFilter::try_new(&config.logging.level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
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

    let launcher = ServerLauncher {
        config,
        bootstrap,
    };
    let handle = launcher.launch().await?;
    tracing::info!(
        bound = ?handle.bound_addrs.iter().filter_map(|a| *a).collect::<Vec<_>>(),
        "shamir-server ready",
    );

    // SIGINT-aware wait. (Windows: Ctrl-C; Unix: SIGINT.)
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");

    handle.shutdown().await;
    Ok(())
}
