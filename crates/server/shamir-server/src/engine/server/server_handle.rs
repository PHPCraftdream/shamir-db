//! Runtime handle for a launched server instance.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::time::Duration;

use shamir_db::{ShamirDb, ShutdownReport};
use shamir_tunables::runtime::RuntimeTunables;
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::audit_appender::FjallAuditAppender;
use crate::db_handler::CancelRefusalAudit;
use crate::http_rest::RestGate;
use crate::scheduler::Scheduler;
use crate::server::server_launcher::audit_teardown::{teardown_audit, AuditTeardown};
use crate::server::store_release_barrier::{release_stores, ReleaseOutcome};

/// Upper bound for [`ServerHandle::shutdown`] to wait for connection tasks to
/// end once they were told to.
const CONN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound for forced connection abort to complete.
const CONN_ABORT_TIMEOUT: Duration = Duration::from_secs(2);

/// Upper bound for [`ServerHandle::shutdown`] to wait for the stores to drop
/// their directory locks (a client that stays connected can hold one).
const STORE_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// One shared budget for quiescing and checkpointing every repository at
/// shutdown. A slow disk (fsync on a loaded runner) must not turn a healthy stop
/// into an unclean report; with the connection drain it stays inside the
/// 30 s outer deadline of `runtime::serve`.
const REPO_FLUSH_BUDGET: Duration = Duration::from_secs(15);

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

/// R4b-1 gauge-binding test seam (`cfg(test)` only — the field is absent
/// from production builds and the type is never constructed outside
/// `ServerLauncher::launch`'s `#[cfg(test)]` capture): the production-boot
/// artifacts the observability activity-gauge source closes over.
///
/// Exposed so `server_launcher/tests` can drive the REAL
/// [`ShamirDbHandler`](crate::db_handler::ShamirDbHandler) the
/// launcher built (with the launcher's own `ByteBudget` and server-wide
/// `SessionStore`) and scrape the REAL `/metrics` endpoint through the
/// actual bound observability address — proving the boot path's
/// `set_activity_source` wiring, not a separately-constructed fixture.
#[cfg(test)]
pub(in crate::engine::server) struct ActivityGaugeTestSeam {
    /// The concrete request handler the launcher built and handed to every
    /// listener — the same instance the activity source reads mirrors from.
    pub handler: Arc<crate::db_handler::ShamirDbHandler>,
    /// The launcher's RI-15 in-flight response-byte budget (shared with the
    /// handler) — a test can hold it fully to park real request futures.
    pub byte_budget: crate::byte_budget::ByteBudget,
    /// The server-wide session store the handler was given via
    /// `with_session_store`.
    pub session_store: Arc<shamir_connect::server::session::SessionStore>,
    pub audit_chain: Arc<shamir_connect::server::audit_chain::AuditChain>,
    /// The address the observability HTTP server ACTUALLY bound (config
    /// may say port 0; this is `ObservabilityHandle::bound_addr`).
    pub observability_bound_addr: std::net::SocketAddr,
    /// ARGON2-GATE (test-only): the boot's Argon2 semaphore itself — a
    /// fixture can pre-occupy the pool through it to force a typed busy
    /// refusal in the `CreateScramUser` handler, and observe it free again
    /// after. Shares the exact `Arc` the derivation gate wraps.
    pub argon2_sem: Arc<shamir_connect::server::argon2_semaphore::Argon2Semaphore>,
}

/// Owner of the runtime state of a launched server.
pub struct ServerHandle {
    /// Live token service, including token mirror and GC lifecycle.
    pub token_service: Arc<crate::security::tokens::TokenService>,
    /// Addresses the server actually bound, in the same order as
    /// `config.listeners`. `None` entries correspond to either a skipped
    /// listener (an unsupported `(kind, profile)` combination — see the
    /// `AcceptPath` match in `server_launcher/launch.rs`) OR a `kind: unix`
    /// listener, which has no `SocketAddr` to report — see
    /// [`Self::bound_ipc_paths`] for that case.
    pub bound_addrs: Vec<Option<SocketAddr>>,
    /// Companion to [`Self::bound_addrs`], same length/order: `Some(path)`
    /// for every `kind: unix` listener (the filesystem path / pipe name it
    /// bound to), `None` for every other listener.
    pub bound_ipc_paths: Vec<Option<String>>,
    /// HTTP REST gate maintenance workers, retained until connections have drained.
    pub(super) http_gates: Vec<Arc<RestGate>>,
    /// Per-listener accept-loop join handles.
    pub(super) listener_tasks: Vec<JoinHandle<()>>,
    /// Background task scheduler.
    pub(super) scheduler: Scheduler,
    /// Audit-log appender (drained on shutdown).
    pub(super) audit_appender: Arc<FjallAuditAppender>,
    pub(super) cancel_refusal_audit: Arc<CancelRefusalAudit>,
    #[cfg(test)]
    pub(super) refusal_drain_pending: Option<tokio::sync::oneshot::Sender<()>>,
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
    /// Dropping the handle also signals this task; only graceful shutdown joins it.
    pub(super) backup_schedule_task: Option<(DropGuard, JoinHandle<()>)>,
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
    pub(super) conn_tasks: crate::connection::ConnTasks,
    /// Outcome of the boot-time format-stamp check (release-1 R1a,
    /// design 3.2): what the check did to `data_dir` before any store
    /// opened (created / adopted / unchanged / version-updated).
    pub(super) format_stamp: shamir_sync::format_stamp::StampOutcome,
    /// RAII single-instance guard: an advisory OS file lock on
    /// `<data_dir>/.shamir.lock`. The kernel releases it automatically
    /// when this `File` is dropped (or the process crashes), so a second
    /// `shamir-server` that tries to open the same `data_dir` gets
    /// `BootError::AlreadyRunning` instead of silently corrupting the
    /// redb stores. Do NOT explicitly unlock — drop is the release.
    /// `None` once [`ServerHandle::take_shutdown_owner`] moved it out for the
    /// embedding path.
    pub(super) data_dir_lock: Option<std::fs::File>,
    /// Instance-level runtime tunables. Initialized from `instance_defaults`
    /// consts — reads are a single atomic load (instant, non-blocking).
    /// Consumer wiring to accept-loop sleep sites is deferred to a follow-up
    /// slice to keep this slice small and behaviour-identical.
    pub tunables: Arc<RuntimeTunables>,
    /// R4b-1 gauge-binding test seam (see [`ActivityGaugeTestSeam`]).
    /// `None` in production builds entirely (`#[cfg(test)]` field).
    #[cfg(test)]
    pub(in crate::engine::server) activity_test_seam: Option<ActivityGaugeTestSeam>,
}

/// cancel-safe: yes — timeout cancels only the wait; abort_all is idempotent.
pub(crate) async fn drain_connections(
    tasks: &crate::connection::ConnTasks,
    drain: Duration,
    abort: Duration,
    report: &mut ShutdownReport,
) -> bool {
    if tokio::time::timeout(drain, tasks.wait_idle()).await.is_ok() {
        return true;
    }

    let live = tasks.live();
    tasks.abort_all();
    let joined = tokio::time::timeout(abort, tasks.wait_idle()).await.is_ok();
    report.add_error(format!(
        "connection drain exceeded {drain:?}; aborted {live} connection task(s){}",
        if joined {
            ""
        } else {
            "; still running after abort bound"
        },
    ));
    tracing::error!(
        live,
        joined,
        ?drain,
        ?abort,
        "shutdown: connection drain timed out"
    );
    joined
}

/// The shutdown state that must OUTLIVE the bounded shutdown wait.
///
/// `runtime::serve` wraps the shutdown future in a deadline; on expiry that
/// future is dropped, and with it anything it still owns. These two must not
/// be dropped there: the scheduled-backup worker is a native thread that keeps
/// doing filesystem work under the managed roots (copying, retention's tagging
/// and deletions) after its last cancellation checkpoint, and the
/// single-instance lock must not be released while it does — an embedding
/// process would otherwise see I/O and a free data dir that outlive `serve`.
///
/// The owner is therefore extracted BEFORE the deadline and BORROWED into the
/// shutdown future (`ServerHandle::shutdown_with_report` takes `&mut Self`
/// here), never moved into it: on a normal shutdown the worker is awaited in
/// place at its original teardown position (before the repository flush), and
/// on expiry the dropped future leaves the worker and the lock intact for
/// [`Self::drain`] to finish afterwards. The deadline still marks the report
/// unclean; it only bounds the audit/repository teardown.
pub(crate) struct ShutdownOwner {
    schedule_task: Option<(DropGuard, JoinHandle<()>)>,
    data_dir_lock: Option<std::fs::File>,
}

/// One-shot probe fired immediately before the owner starts awaiting the
/// scheduled worker's completion signal. Test-only: a shutdown test that
/// parks the worker waits for this marker instead of guessing how long the
/// earlier teardown stages take, so "shutdown must not return while the
/// worker is parked" can never pass vacuously on a slow runner.
#[cfg(test)]
static SCHEDULED_JOIN_PROBE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// Clears an unfired [`arm_scheduled_join_probe`] on drop; a probe that fires
/// removes itself from the slot.
#[cfg(test)]
pub(crate) struct ScheduledJoinProbeGuard;

#[cfg(test)]
impl Drop for ScheduledJoinProbeGuard {
    fn drop(&mut self) {
        *SCHEDULED_JOIN_PROBE.lock().expect("scheduled join probe") = None;
    }
}

#[cfg(test)]
pub(crate) fn arm_scheduled_join_probe(
    probe: impl FnOnce() + Send + 'static,
) -> ScheduledJoinProbeGuard {
    *SCHEDULED_JOIN_PROBE.lock().expect("scheduled join probe") = Some(Box::new(probe));
    ScheduledJoinProbeGuard
}

#[cfg(test)]
fn fire_scheduled_join_probe() {
    let probe = SCHEDULED_JOIN_PROBE
        .lock()
        .expect("scheduled join probe")
        .take();
    if let Some(probe) = probe {
        probe();
    }
}

/// One-shot probe fired immediately BEFORE the ordered shutdown's repository
/// flush. A shutdown test compares its order with [`arm_scheduled_join_probe`]:
/// the scheduled worker and its in-flight run are joined BEFORE the flush, so
/// the join marker must fire first (mutant: the worker awaited after the
/// flush).
#[cfg(test)]
static REPO_FLUSH_PROBE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// Clears an unfired [`arm_repo_flush_probe`]; a probe that fires removes
/// itself.
#[cfg(test)]
pub(crate) struct RepoFlushProbeGuard;

#[cfg(test)]
impl Drop for RepoFlushProbeGuard {
    fn drop(&mut self) {
        *REPO_FLUSH_PROBE.lock().expect("repo flush probe") = None;
    }
}

/// Arm ONE repository-flush probe.
#[cfg(test)]
pub(crate) fn arm_repo_flush_probe(probe: impl FnOnce() + Send + 'static) -> RepoFlushProbeGuard {
    *REPO_FLUSH_PROBE.lock().expect("repo flush probe") = Some(Box::new(probe));
    RepoFlushProbeGuard
}

/// Fire and consume the repository-flush probe, if any.
#[cfg(test)]
fn fire_repo_flush_probe() {
    let probe = REPO_FLUSH_PROBE.lock().expect("repo flush probe").take();
    if let Some(probe) = probe {
        probe();
    }
}

impl ShutdownOwner {
    /// Await the scheduled worker in place and forget its handle.
    ///
    /// Called from INSIDE the bounded shutdown future with `&mut self`, so the
    /// owner itself is never moved into — or dropped with — that future. This
    /// keeps the original teardown position: the scheduled task and its
    /// in-flight run are awaited BEFORE the repository flush, not after it.
    ///
    /// Deliberately unbounded: a run already past its last cooperative
    /// cancellation checkpoint has to finish its own cleanup. The wait is a
    /// COMPLETION signal (the runner's per-run channel), not a join of the
    /// native runner thread.
    async fn join_schedule(&mut self, report: &mut ShutdownReport) {
        let Some((_, task)) = self.schedule_task.as_mut() else {
            return;
        };
        // Awaited THROUGH `&mut`: while the wait is pending the handle and its
        // drop guard stay in the owner, so the deadline dropping this future
        // mid-wait leaves them for `drain` instead of detaching the worker.
        #[cfg(test)]
        fire_scheduled_join_probe();
        let joined = task.await;
        // Finished: forget the handle (the guard drops with it).
        self.schedule_task = None;
        if let Err(error) = joined {
            report.add_error(format!("backup schedule task failed: {error}"));
        }
    }

    /// Await whatever the deadline left unawaited, then release the instance
    /// lock by drop.
    ///
    /// On a normal shutdown [`Self::join_schedule`] already ran inside the
    /// deadline, so this only releases the lock. On expiry the shutdown future
    /// was dropped mid-teardown and the worker is still awaited here — the lock
    /// is released afterwards, never before.
    pub(crate) async fn drain(mut self, report: &mut ShutdownReport) {
        self.join_schedule(report).await;
        // Released here, after the scheduled worker finished — never
        // explicitly unlocked (drop is the release).
        drop(self.data_dir_lock.take());
    }
}

/// Store-root names for the release report: bounded tokens, never full paths.
fn store_names(locked: &[std::path::PathBuf]) -> String {
    locked
        .iter()
        .filter_map(|dir| dir.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

impl ServerHandle {
    /// What the boot-time format-stamp check did to `data_dir`.
    pub fn format_stamp(&self) -> shamir_sync::format_stamp::StampOutcome {
        self.format_stamp
    }

    /// Stop accepting new connections, then drain the scheduler + audit log.
    /// Returns repository-level checkpoint failures; ignored reports preserve
    /// the existing caller behaviour. Connection drain remains bounded.
    pub async fn shutdown(mut self) -> ShutdownReport {
        let mut report = self.pending_shutdown_report();
        let mut owner = self.take_shutdown_owner();
        self.shutdown_with_report(&mut report, &mut owner).await;
        crate::observability::record_shutdown(&report);
        report
    }

    pub(crate) fn pending_shutdown_report(&self) -> ShutdownReport {
        self.shamir.shutdown_report()
    }

    /// Move the scheduled worker and the instance lock out of the handle.
    ///
    /// `runtime::serve` calls this BEFORE it wraps the shutdown future in its
    /// deadline, so expiry can abandon the audit/repository teardown without
    /// dropping a running native backup thread or releasing the data directory.
    /// Callers that keep the handle until it is consumed (`shutdown`,
    /// `shutdown_with_report`) are unaffected: they still join the worker and
    /// drop the lock themselves.
    pub(crate) fn take_shutdown_owner(&mut self) -> ShutdownOwner {
        ShutdownOwner {
            schedule_task: self.backup_schedule_task.take(),
            data_dir_lock: self.data_dir_lock.take(),
        }
    }

    /// cancel-safe: NO — joins and flushes may be incomplete; caller retains
    /// repository outcomes and marks deadline expiry as unclean. `owner` is
    /// borrowed, not taken: dropping this future must never drop the
    /// scheduled worker or the instance lock.
    pub(crate) async fn shutdown_with_report(
        self,
        report: &mut ShutdownReport,
        owner: &mut ShutdownOwner,
    ) {
        // 1. Tell all accept loops to stop. CancellationToken::cancel() is
        //    persistent: every existing `.cancelled().await` resolves on the
        //    next poll, and any new caller sees the cancelled state
        //    immediately. No subscribe-window race.
        self.cancel_refusal_audit.close();
        self.shutdown_token.cancel();
        // 2. Wait for accept loops to finish, then wait for tracked accepted
        // tasks to quiesce before any repository flush.
        let listener_deadline = tokio::time::Instant::now() + CONN_DRAIN_TIMEOUT;
        let mut quiesced = true;
        for mut task in self.listener_tasks {
            let abort = task.abort_handle();
            match tokio::time::timeout_at(listener_deadline, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    report.add_error(format!("listener task failed: {error}"));
                    quiesced = false;
                }
                Err(_) => {
                    abort.abort();
                    let joined = tokio::time::timeout(CONN_ABORT_TIMEOUT, &mut task)
                        .await
                        .is_ok();
                    report.add_error(format!(
                        "HTTP listener drain exceeded {CONN_DRAIN_TIMEOUT:?}; aborted"
                    ));
                    quiesced &= joined;
                }
            }
        }
        // One drain budget for listeners and accepted connections together, so
        // the stage stays inside the runtime shutdown deadline.
        let remaining = listener_deadline.saturating_duration_since(tokio::time::Instant::now());
        quiesced &=
            drain_connections(&self.conn_tasks, remaining, CONN_ABORT_TIMEOUT, report).await;
        // Gates stop after transport and connection tasks drain, so the final
        // audit flush includes their notices.
        for gate in &self.http_gates {
            gate.shutdown().await;
        }
        drop(self.http_gates);
        self.token_service.shutdown_gc().await;
        // 2b. Stop the follower-replication supervisor: cancel every running
        //     follower loop, then join the reconcile-tick task (already told
        //     to stop by the root `shutdown_token.cancel()` above) so it drops
        //     its supervisor reference. Done after the accept loops stop (no
        //     new subscription rows can arrive) and before flush.
        self.repl_supervisor.stop_all().await;
        if let Some(task) = self.repl_supervisor_task {
            let _ = task.handle.await;
        }
        // 3. Drain the refusal-audit queue and the audit chain under ONE
        //    shared deadline: the scheduler is signalled before it is joined,
        //    the appender's final drain + anchor write runs on a blocking
        //    thread, and a stalled checkpoint or flusher marks the report
        //    unclean instead of stalling the shutdown future. Teardown
        //    continues to the repository flush either way; a timeout does not
        //    cancel an in-flight fsync.
        teardown_audit(
            AuditTeardown {
                cancel_refusal_audit: &self.cancel_refusal_audit,
                appender: &self.audit_appender,
                scheduler: self.scheduler,
                #[cfg(test)]
                drain_probe: self.refusal_drain_pending,
            },
            report,
        )
        .await;
        // Signal the observability server to stop BEFORE the wait a deadline can
        // abandon (the scheduled join below is deliberately unbounded inside the
        // bounded future). Its own `Drop` cancels the same token, so an expiry
        // that drops this future cannot leave a poller and a router holding the
        // database; the full await stays in step 5.
        if let Some(obs) = &self.observability {
            obs.cancel();
        }
        // Do not abort: an in-flight copy or retention operation must finish
        // cleanup. This is the ORIGINAL position — before the repository flush
        // — and it borrows the owner, so a deadline can abandon this teardown
        // without abandoning the worker (`ShutdownOwner::drain` then awaits it
        // after the deadline).
        owner.join_schedule(report).await;
        // 4. Flush all repo MemBuffers to durable backing so buffered
        #[cfg(test)]
        fire_repo_flush_probe();
        //    commits in the last ~500 ms window are not lost on graceful
        //    stop. Must happen AFTER accept loops have stopped (no new
        //    writes can arrive) and BEFORE redb locks are released.
        if !quiesced {
            report.add_error("repository flush skipped: connection tasks still alive".to_string());
            tracing::error!(
                live = self.conn_tasks.live(),
                "shutdown: repository flush skipped because connection tasks remain",
            );
        } else {
            self.shamir
                .flush_all_report(REPO_FLUSH_BUDGET, report)
                .await;
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
        // 9. Release the stores: drop this handle's own references, then wait
        //    until fjall has dropped every directory lock. Detached tasks
        //    (buffered-store flushers, connection tasks) can outlive the
        //    steps above; returning before they finish made an immediate
        //    relaunch of the same data dir fail with `Locked`. The
        //    single-instance `data_dir_lock` is released last, on return — or
        //    by the embedding caller when [`ServerHandle::take_shutdown_owner`]
        //    owns it.
        let data_dir = self.data_dir;
        // The test seam holds `Arc<ShamirDbHandler>` (and the handler holds the
        // DB), so a fixture that never takes it would keep its OWN stores
        // locked through the barrier and turn a healthy shutdown into an
        // unclean report. It is test-only state: drop it with the handles.
        #[cfg(test)]
        drop(self.activity_test_seam);
        drop(self.shamir);
        drop(self.audit_appender);
        drop(self.repl_supervisor);
        drop(self.token_service);
        // The barrier's outcome belongs in the report, not only in a log line,
        // and the two failures stay distinct: a lock that is still HELD names
        // the store, while a probe that could not run (a saturated blocking
        // pool) says that the release could not be verified at all.
        match release_stores(&data_dir, STORE_RELEASE_TIMEOUT).await {
            ReleaseOutcome::Released => {}
            ReleaseOutcome::StillLocked(still_locked) => {
                let stores = store_names(&still_locked);
                tracing::warn!(
                    ?still_locked,
                    timeout = ?STORE_RELEASE_TIMEOUT,
                    "shutdown: stores still held after the release timeout",
                );
                report.add_error(format!(
                    "stores still held after the release timeout {STORE_RELEASE_TIMEOUT:?}: \
                     {stores} — a writer outside this teardown still holds the store (a detached \
                     task or an external process); an immediate relaunch of the same data \
                     directory can fail with `Locked`"
                ));
            }
            ReleaseOutcome::Unverified => {
                tracing::warn!(
                    timeout = ?STORE_RELEASE_TIMEOUT,
                    "shutdown: store release could not be verified in time",
                );
                report.add_error(format!(
                    "store release could not be verified within {STORE_RELEASE_TIMEOUT:?}: the \
                     blocking pool did not run the lock probe, so whether every store released \
                     its directory lock is UNKNOWN; an immediate relaunch of the same data \
                     directory can fail with `Locked`"
                ));
            }
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
    /// can assert the boot-time wiring in `server_launcher/launch.rs` flipped the
    /// toggle that every `StartMigration` handler reads.
    pub fn experimental_migration_enabled(&self) -> bool {
        self.shamir.experimental_migration_enabled()
    }
}
