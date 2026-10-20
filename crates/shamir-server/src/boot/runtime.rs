//! Runtime modes — the reusable `serve` core and foreground shutdown signal.
//!
//! Every runtime mode (foreground, Windows service, Linux systemd) funnels
//! into [`serve`], which boots a [`ServerLauncher`] and waits for an
//! arbitrary shutdown future. Only the shutdown trigger differs between
//! modes; the boot + graceful-drain logic is identical.

use std::future::Future;
use std::pin::Pin;

use shamir_db::ShutdownReport;

use crate::config::Config;
use crate::server::{BootstrapMode, ServerLauncher};

/// Maximum time to wait for in-flight connections to drain after receiving
/// a shutdown signal. If the deadline expires, we return an unclean report
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
/// connection), we return an unclean report rather than blocking
/// indefinitely.
pub async fn serve(
    config: Config,
    bootstrap: BootstrapMode,
    shutdown: impl std::future::Future<Output = ()>,
    on_ready: impl FnOnce(),
) -> anyhow::Result<ShutdownReport> {
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

    let mut report = handle.pending_shutdown_report();
    let report = finish_shutdown(SHUTDOWN_DEADLINE, &mut report, |report| {
        Box::pin(handle.shutdown_with_report(report))
    })
    .await;
    crate::observability::record_shutdown(&report);
    Ok(report)
}

/// cancel-safe: NO — expiry abandons unfinished teardown, not completed outcomes.
pub(crate) async fn finish_shutdown(
    deadline: std::time::Duration,
    report: &mut ShutdownReport,
    shutdown: impl for<'a> FnOnce(
        &'a mut ShutdownReport,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
) -> ShutdownReport {
    if tokio::time::timeout(deadline, shutdown(report))
        .await
        .is_err()
    {
        report.add_error("graceful shutdown exceeded deadline — forcing exit");
    }
    report.clone()
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
        let _ = sd_notify::notify(&[sd_notify::NotifyState::Ready]);
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
