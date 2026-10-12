//! Runtime handle for a launched server instance.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use shamir_db::{ShamirDb, ShutdownReport};
use shamir_tunables::runtime::RuntimeTunables;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::audit_appender::FjallAuditAppender;
use crate::conn_limiter::ConnLimiter;
use crate::scheduler::Scheduler;
use crate::server::store_release_barrier::wait_stores_released;

/// Upper bound for [`ServerHandle::shutdown`] to wait for connection tasks to
/// end once they were told to.
const CONN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound for [`ServerHandle::shutdown`] to wait for the stores to drop
/// their directory locks (a client that stays connected can hold one).
const STORE_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Handle for the periodic meta-snapshot task. Just the `JoinHandle` — the
/// stop signal is the shared root `shutdown_token` (cancelled by
/// [`ServerHandle::shutdown`] before this handle is awaited), so no per-task
/// stop channel is needed.
pub(super) struct MetaSnapshotTask {
    pub(super) handle: JoinHandle<()>,
}

/// Handle for the periodic follower-replication reconcile-tick task (386-c).
/// Stopped via the shared root `shutdown_token`; the `JoinHandle` is awaited
/// on shutdown so the task drops its `Arc<SubscriptionSupervisor>` reference.
pub struct ReplSupervisorTask {
    pub(super) handle: JoinHandle<()>,
}

/// Owner of the runtime state of a launched server.
pub struct ServerHandle {
    /// Addresses the server actually bound, in the same order as
    /// `config.listeners`. `None` entries correspond to either a skipped
    /// listener (an unsupported `(kind, profile)` combination — see the
    /// `AcceptPath` match in `server_launcher.rs`) OR a `kind: unix`
    /// listener, which has no `SocketAddr` to report — see
    /// [`Self::bound_ipc_paths`] for that case.
    pub bound_addrs: Vec<Option<SocketAddr>>,
    /// Companion to [`Self::bound_addrs`], same length/order: `Some(path)`
    /// for every `kind: unix` listener (the filesystem path / pipe name it
    /// bound to), `None` for every other listener.
    pub bound_ipc_paths: Vec<Option<String>>,
    /// Per-listener accept-loop join handles.
    pub(super) listener_tasks: Vec<JoinHandle<()>>,
    /// Background task scheduler.
    pub(super) scheduler: Scheduler,
    /// Audit-log appender (drained on shutdown).
    pub(super) audit_appender: Arc<FjallAuditAppender>,
    /// Cancellation token signalling shutdown to every accept loop.
    ///
    /// Why CancellationToken, not `Notify::notify_waiters`: Notify is
    /// lossy across the subscribe-window. `select!` polls `shutdown.notified()`
    /// — that creates a *new* Notified future on every poll. Between two
    /// polls there is no live waiter; a `notify_waiters()` that lands there
    /// is silently dropped. Result: accept loop hangs forever in
    /// `listener.accept().await`, observed at cold-start ~10% of the time
    /// on this codebase. CancellationToken sets a persistent flag; any
    /// future `.cancelled().await` resolves immediately if cancel was
    /// already set, so the race is closed by construction.
    pub(super) shutdown_token: CancellationToken,
    /// Optional observability HTTP server.
    pub(super) observability: Option<crate::observability::ObservabilityHandle>,
    /// Periodic meta-snapshot task (M2 — persistent lockout + rate-limit
    /// state). One 60s tick persists BOTH the lockout store and the
    /// rate-limiter buckets. Awaited on shutdown so the redb file lock on
    /// `server_meta.redb` is released cleanly before a same-data-dir
    /// restart can succeed.
    pub(super) meta_snapshot_task: Option<MetaSnapshotTask>,
    /// Phase B Stage 6 — periodic reaper for expired interactive txs.
    /// Drained on shutdown so the `Arc<TxRegistry>` reference held by the
    /// task drops and any lingering open txs are dropped (RAII abort).
    pub(super) interactive_tx_reaper: Option<crate::tx_registry::ReaperTask>,
    /// FG-5b — periodic reaper for idle-timeout-expired result cursors.
    /// Drained on shutdown so the `Arc<CursorRegistry>` reference held by
    /// the task drops and any lingering open cursors release their pinned
    /// `SnapshotGuard`s (RAII unpin of MVCC GC).
    pub(super) cursor_reaper: Option<crate::cursor_registry::CursorReaperTask>,
    /// Follower-replication supervisor (386-c). Held here so its follower
    /// loops survive past boot (dropping it would kill every loop). On
    /// shutdown `stop_all()` cancels every running loop before the tasks are
    /// joined.
    pub(super) repl_supervisor: Arc<crate::replication::SubscriptionSupervisor>,
    /// Periodic reconcile-tick task driving [`Self::repl_supervisor`]. Cancelled
    /// by the root `shutdown_token` and joined on shutdown.
    pub(super) repl_supervisor_task: Option<ReplSupervisorTask>,
    /// ShamirDb reference — needed on shutdown to drain all repo
    /// MemBuffers to durable backing, closing the ~500 ms buffered-
    /// commit loss window on graceful stop.
    pub(super) shamir: Arc<ShamirDb>,
    /// Data dir the stores live under — scanned by the shutdown barrier that
    /// waits for their directory locks to be released.
    pub(super) data_dir: PathBuf,
    /// Live-connection tracker: shutdown waits on it until every connection
    /// task (told to end through `shutdown_token`) has dropped its handler
    /// context and, with it, its references to the stores.
    pub(super) conn_limiter: ConnLimiter,
    /// RAII single-instance guard: an advisory OS file lock on
    /// `<data_dir>/.shamir.lock`. The kernel releases it automatically
    /// when this `File` is dropped (or the process crashes), so a second
    /// `shamir-server` that tries to open the same `data_dir` gets
    /// `BootError::AlreadyRunning` instead of silently corrupting the
    /// redb stores. Do NOT explicitly unlock — drop is the release.
    pub(super) _data_dir_lock: std::fs::File,
    /// Instance-level runtime tunables. Initialized from `instance_defaults`
    /// consts — reads are a single atomic load (instant, non-blocking).
    /// Consumer wiring to accept-loop sleep sites is deferred to a follow-up
    /// slice to keep this slice small and behaviour-identical.
    pub tunables: Arc<RuntimeTunables>,
}

impl ServerHandle {
    /// Stop accepting new connections, then drain the scheduler + audit log.
    /// Returns repository-level checkpoint failures; ignored reports preserve
    /// the existing caller behaviour. Connection drain remains bounded.
    pub async fn shutdown(self) -> ShutdownReport {
        let mut report = self.pending_shutdown_report();
        self.shutdown_with_report(&mut report).await;
        crate::observability::record_shutdown(&report);
        report
    }

    pub(crate) fn pending_shutdown_report(&self) -> ShutdownReport {
        self.shamir.shutdown_report()
    }

    /// cancel-safe: NO — joins and flushes may be incomplete; caller retains
    /// repository outcomes and marks deadline expiry as unclean.
    pub(crate) async fn shutdown_with_report(self, report: &mut ShutdownReport) {
        // 1. Tell all accept loops to stop. CancellationToken::cancel() is
        //    persistent: every existing `.cancelled().await` resolves on the
        //    next poll, and any new caller sees the cancelled state
        //    immediately. No subscribe-window race.
        self.shutdown_token.cancel();
        // 2. Wait for accept loops to finish.
        for task in self.listener_tasks {
            let _ = task.await;
        }
        // 2a. Connection tasks watch the same token (see
        //     `run_until_cancelled`); wait until the last one dropped its
        //     handler context, which holds references to the stores.
        if tokio::time::timeout(CONN_DRAIN_TIMEOUT, self.conn_limiter.wait_idle())
            .await
            .is_err()
        {
            tracing::warn!(
                live = self.conn_limiter.live(),
                timeout = ?CONN_DRAIN_TIMEOUT,
                "shutdown: connections still alive after the drain timeout",
            );
        }
        // 2b. Stop the follower-replication supervisor: cancel every running
        //     follower loop, then join the reconcile-tick task (already told
        //     to stop by the root `shutdown_token.cancel()` above) so it drops
        //     its supervisor reference. Done after the accept loops stop (no
        //     new subscription rows can arrive) and before flush.
        self.repl_supervisor.stop_all().await;
        if let Some(task) = self.repl_supervisor_task {
            let _ = task.handle.await;
        }
        // 3. Drain the audit chain + scheduler.
        self.audit_appender.shutdown().await;
        self.scheduler.shutdown().await;
        // 4. Flush all repo MemBuffers to durable backing so buffered
        //    commits in the last ~500 ms window are not lost on graceful
        //    stop. Must happen AFTER accept loops have stopped (no new
        //    writes can arrive) and BEFORE redb locks are released.
        self.shamir
            .flush_all_report(Duration::from_secs(5), report)
            .await;
        // 5. Stop the observability HTTP server (if any).
        if let Some(obs) = self.observability {
            obs.shutdown().await;
        }
        // 6. Join the meta-snapshot task. It was already told to stop by the
        //    `shutdown_token.cancel()` at the top of this method (it holds a
        //    clone of the root token). Awaiting its handle lets it drop its
        //    Arc<ServerMetaStore> reference so the redb file lock on
        //    `server_meta.redb` releases for a same-data-dir restart. The
        //    task writes one final lockout + rate-limit snapshot inside its
        //    cancel branch (best-effort) before exiting.
        if let Some(snap) = self.meta_snapshot_task {
            let _ = snap.handle.await;
        }
        // 7. Join the interactive-tx reaper (also cancelled by the root token
        //    above) so its Arc<TxRegistry> drops and any lingering open txs
        //    are dropped (RAII abort).
        if let Some(reaper) = self.interactive_tx_reaper {
            let _ = reaper.handle.await;
        }
        // 8. Join the cursor reaper (FG-5b, also cancelled by the root
        //    token above) so its Arc<CursorRegistry> drops and any
        //    lingering open cursors release their pinned SnapshotGuards.
        if let Some(reaper) = self.cursor_reaper {
            let _ = reaper.handle.await;
        }
        // 9. Release the stores: drop this handle's own references, then wait
        //    until fjall has dropped every directory lock. Detached tasks
        //    (buffered-store flushers, connection tasks) can outlive the
        //    steps above; returning before they finish made an immediate
        //    relaunch of the same data dir fail with `Locked`. The
        //    single-instance `_data_dir_lock` is released last, on return.
        let data_dir = self.data_dir;
        drop(self.shamir);
        drop(self.audit_appender);
        drop(self.repl_supervisor);
        if let Err(still_locked) = wait_stores_released(&data_dir, STORE_RELEASE_TIMEOUT).await {
            tracing::warn!(
                ?still_locked,
                timeout = ?STORE_RELEASE_TIMEOUT,
                "shutdown: stores still held after the release timeout",
            );
        }
    }

    /// Returns the first bound TCP+TLS-exporter address — useful for
    /// integration tests that just want "where do I connect?".
    pub fn first_tls_exporter_addr(&self) -> Option<SocketAddr> {
        self.bound_addrs.iter().filter_map(|a| *a).next()
    }

    /// Returns the first bound local-IPC path (Unix socket path / Windows
    /// pipe name) — the `bound_ipc_paths` analogue of
    /// [`Self::first_tls_exporter_addr`], for tests that just want "where
    /// do I connect for the `kind: unix` listener?".
    pub fn first_ipc_path(&self) -> Option<&str> {
        self.bound_ipc_paths.iter().find_map(|p| p.as_deref())
    }

    /// Whether the experimental online storage-migration API
    /// (`StartMigration`/`CommitMigration`/`RollbackMigration`/
    /// `MigrationStatus`) is enabled on this booted server. `true` only when
    /// `security.enable_experimental_migration_api` was set in the config at
    /// boot — the default is `false`. Exposed primarily so integration tests
    /// can assert the boot-time wiring in `server_launcher.rs` flipped the
    /// toggle that every `StartMigration` handler reads.
    pub fn experimental_migration_enabled(&self) -> bool {
        self.shamir.experimental_migration_enabled()
    }
}
