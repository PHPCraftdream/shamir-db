//! Runtime handle for a launched server instance.

use std::net::SocketAddr;
use std::sync::Arc;

use shamir_db::ShamirDb;
use shamir_tunables::runtime::RuntimeTunables;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::audit_appender::FjallAuditAppender;
use crate::scheduler::Scheduler;

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
    /// `config.listeners`. `None` entries correspond to skipped listeners
    /// (WS / plain are not yet supported by this MVP boot path).
    pub bound_addrs: Vec<Option<SocketAddr>>,
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
    /// In-flight per-connection tasks are NOT awaited explicitly — they
    /// finish on their own once their TcpStreams close.
    pub async fn shutdown(self) {
        // 1. Tell all accept loops to stop. CancellationToken::cancel() is
        //    persistent: every existing `.cancelled().await` resolves on the
        //    next poll, and any new caller sees the cancelled state
        //    immediately. No subscribe-window race.
        self.shutdown_token.cancel();
        // 2. Wait for accept loops to finish.
        for task in self.listener_tasks {
            let _ = task.await;
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
        if let Err(e) = self.shamir.flush_all().await {
            tracing::warn!("shutdown: flush_all failed: {}", e);
        }
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
    }

    /// Returns the first bound TCP+TLS-exporter address — useful for
    /// integration tests that just want "where do I connect?".
    pub fn first_tls_exporter_addr(&self) -> Option<SocketAddr> {
        self.bound_addrs.iter().filter_map(|a| *a).next()
    }
}
