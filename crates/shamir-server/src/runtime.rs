//! Runtime modes — the reusable `serve` core and foreground shutdown signal.
//!
//! Every runtime mode (foreground, Windows service, Linux systemd) funnels
//! into [`serve`], which boots a [`ServerLauncher`] and waits for an
//! arbitrary shutdown future. Only the shutdown trigger differs between
//! modes; the boot + graceful-drain logic is identical.

use crate::config::Config;
use crate::server::{BootstrapMode, ServerLauncher};

/// Boot the server, wait for `shutdown`, then drain gracefully.
///
/// This is the single entry point every runtime mode reuses. The caller
/// supplies the shutdown trigger — a `oneshot` receiver in tests,
/// [`foreground_shutdown`] in production, a SCM Stop event on Windows, etc.
pub async fn serve(
    config: Config,
    bootstrap: BootstrapMode,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher
        .launch()
        .await
        .map_err(|e| anyhow::anyhow!("server boot failed: {e}"))?;

    tracing::info!(
        bound = ?handle.bound_addrs.iter().filter_map(|a| *a).collect::<Vec<_>>(),
        "shamir-server ready",
    );

    shutdown.await;
    tracing::info!("shutting down");
    handle.shutdown().await;
    Ok(())
}

/// Resolves when the OS asks us to stop: Ctrl+C everywhere, plus SIGTERM on
/// unix (systemd / `kill` default).
pub async fn foreground_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
