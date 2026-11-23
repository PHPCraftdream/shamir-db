//! Runtime modes — the reusable `serve` core and foreground shutdown signal.
//!
//! Every runtime mode (foreground, Windows service, Linux systemd) funnels
//! into [`serve`], which boots a [`ServerLauncher`] and waits for an
//! arbitrary shutdown future. Only the shutdown trigger differs between
//! modes; the boot + graceful-drain logic is identical.

#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::pin::Pin;

use shamir_db::ShutdownReport;

use crate::config::Config;
use crate::server::{BootstrapMode, ServerHandle, ServerLauncher};

/// Maximum time to wait for in-flight connections to drain after receiving
/// a shutdown signal. If the deadline expires, we return an unclean report.
///
/// The deadline bounds the whole shutdown future (connection and task drain,
/// the scheduled-worker wait while it is pending, audit and repository
/// teardown, store release): on expiry whichever stage is still running is
/// dropped and the report is marked unclean. The
/// scheduled-backup worker and the data-directory lock are owned outside it
/// (extracted before the deadline, borrowed by the future) and are still
/// awaited before [`serve`] returns. There is no forced exit and nothing calls
/// `std::process::exit`.
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
/// indefinitely. The bound covers the whole shutdown future (connection and
/// task drain, the scheduled-worker wait, audit and repository teardown, store
/// release); the scheduled-backup worker and the data-directory lock are owned outside it
/// and are still awaited before this function returns (there is no forced
/// exit and nothing calls `std::process::exit`).
///
/// Order matters: the scheduled-backup worker and its in-flight run are
/// awaited BEFORE the repository flush (their original position). Only the
/// BOUND excludes them, not the order — a deadline that fires mid-teardown
/// leaves them to the owner, which awaits them after the deadline and releases
/// the data-directory lock only then. The worker wait is a completion signal,
/// not a join of the native runner thread.
pub async fn serve(
    config: Config,
    bootstrap: BootstrapMode,
    shutdown: impl std::future::Future<Output = ()>,
    on_ready: impl FnOnce(),
) -> anyhow::Result<ShutdownReport> {
    let launcher = ServerLauncher::new(config);
    let launcher = ServerLauncher {
        bootstrap,
        ..launcher
    };
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

    let report = drain_server(handle, SHUTDOWN_DEADLINE).await;
    crate::observability::record_shutdown(&report);
    Ok(report)
}

/// The embedding shutdown sequence: one bounded teardown, then the state that
/// must outlive that bound.
///
/// The shutdown future BORROWS the `ShutdownOwner` (`&mut`) instead of
/// owning it, which is what keeps both properties at once: on a normal
/// shutdown the scheduled worker and its in-flight run are awaited at their
/// original position, before the repository flush; on deadline expiry the
/// future is dropped with the owner intact, the report is marked unclean, and
/// the worker is still awaited here before the data-directory lock is released
/// by `ShutdownOwner::drain`.
pub(crate) async fn drain_server(
    handle: ServerHandle,
    deadline: std::time::Duration,
) -> ShutdownReport {
    let mut handle = handle;
    let mut report = handle.pending_shutdown_report();
    // Extracted BEFORE the deadline: expiry drops the teardown future, and a
    // running native backup thread plus the instance lock must survive that.
    // The future borrows the owner (`&mut`), it never takes it.
    let mut owner = handle.take_shutdown_owner();
    let expired = tokio::time::timeout(
        deadline,
        handle.shutdown_with_report(&mut report, &mut owner),
    )
    .await
    .is_err();
    if expired {
        report.add_error(deadline_error(deadline));
    }
    // Outside the deadline: on expiry the teardown was abandoned mid-way, but
    // the worker is still awaited here and the lock is released only afterwards.
    owner.drain(&mut report).await;
    report
}

/// The one truthful deadline message, shared by both shutdown compositions.
///
/// Nothing forces an exit: the exit CODE reports the unclean stop. The message
/// text describes the production composition: `drain_server` awaits the
/// scheduled worker and releases the data-directory lock before the caller
/// returns. The test-only `finish_shutdown` does not by itself carry that
/// guarantee — its closure decides what is still awaited after the deadline.
fn deadline_error(deadline: std::time::Duration) -> String {
    format!(
        "graceful shutdown exceeded deadline {deadline:?}: the teardown stage still running \
         was abandoned; nothing forces an exit, and the scheduled backup worker and the \
         data-directory lock are still awaited before this call returns"
    )
}

#[cfg(test)]
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
        report.add_error(deadline_error(deadline));
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
