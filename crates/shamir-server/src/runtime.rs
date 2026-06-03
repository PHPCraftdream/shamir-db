//! Runtime modes — the reusable `serve` core and foreground shutdown signal.
//!
//! Every runtime mode (foreground, Windows service, Linux systemd) funnels
//! into [`serve`], which boots a [`ServerLauncher`] and waits for an
//! arbitrary shutdown future. Only the shutdown trigger differs between
//! modes; the boot + graceful-drain logic is identical.

use crate::config::Config;
use crate::server::{BootstrapMode, ServerLauncher};

/// Maximum time to wait for in-flight connections to drain after receiving
/// a shutdown signal. If the deadline expires, we log a warning and return
/// so the process exits (the OS reclaims sockets).
const SHUTDOWN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Boot the server, wait for `shutdown`, then drain gracefully.
///
/// This is the single entry point every runtime mode reuses. The caller
/// supplies the shutdown trigger — a `oneshot` receiver in tests,
/// [`foreground_shutdown`] in production, a SCM Stop event on Windows, etc.
///
/// After the shutdown signal fires, the drain phase is bounded by
/// [`SHUTDOWN_DEADLINE`]. If draining exceeds the deadline (e.g. a stuck
/// connection), we log a warning and return rather than blocking
/// indefinitely.
pub async fn serve(
    config: Config,
    bootstrap: BootstrapMode,
    shutdown: impl std::future::Future<Output = ()>,
    on_ready: impl FnOnce(),
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

    on_ready();

    shutdown.await;
    tracing::info!("shutting down");

    // NOTE: The deadline is exercised only via the tokio::time::timeout
    // wrapper below. A full serve()+hung-drain integration test would
    // require a mock ServerHandle; the timeout path is obviously correct
    // and covered by tokio's own test suite.
    match tokio::time::timeout(SHUTDOWN_DEADLINE, handle.shutdown()).await {
        Ok(()) => {}
        Err(_) => tracing::warn!(
            deadline_secs = SHUTDOWN_DEADLINE.as_secs(),
            "graceful shutdown exceeded deadline — forcing exit"
        ),
    }
    Ok(())
}

/// Notify the init system that the server is ready (post-bind).
///
/// On Linux under systemd (`Type=notify`), this sends `READY=1` via
/// `sd_notify`. Everywhere else (macOS, BSD, Windows, plain terminal) it
/// is a compile-time no-op.
pub fn notify_ready() {
    #[cfg(target_os = "linux")]
    {
        // Best-effort; no-op if $NOTIFY_SOCKET is unset (not under systemd).
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
    }
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
