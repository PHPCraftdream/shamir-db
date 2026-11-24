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
use crate::server::store_release_barrier::{
    release_stores_until, ReleaseOutcome, UnverifiedReason,
};

/// Each late-stage WAIT ends at the earlier instant: stage start + its cap, or
/// outer deadline - reserve (500 ms metadata / 250 ms store release). These
/// bound waits only: synchronous work is not interrupted; a timed-out join's
/// task may continue (the metadata join is detached).
const LATE_RESERVE: Duration = Duration::from_secs(1);

const CONN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

const CONN_ABORT_TIMEOUT: Duration = Duration::from_secs(2);

const STORE_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound for joining the periodic meta-snapshot task at teardown step 6.
///
/// The task is told to stop by the root `shutdown_token` at the top of the
/// teardown but still has to run one final lockout + rate-limit snapshot, and
/// that write is submitted to the blocking pool: a saturated or stalled pool
/// keeps the join pending for as long as the pool is starved. The bound turns
/// that into a REPORTED verdict instead of a teardown that never reaches the
/// store-release stage — and, inside an outer shutdown budget, it may only
/// spend what is left of that budget (see [`stage_bound`]).
const META_SNAPSHOT_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Report entry when the meta-snapshot join misses its bound: the periodic
/// metadata snapshot did not settle, so its `Arc<ServerMetaStore>` — and with
/// it the fjall `server_meta` store lock — may still be held. Named separately
/// from the generic deadline message so an outer expiry cannot replace this
/// stage's verdict.
const ERR_META_SNAPSHOT_UNSETTLED: &str = "meta snapshot task did not stop within";

/// Shared budget for repository shutdown quiescing and checkpointing.
const REPO_FLUSH_BUDGET: Duration = Duration::from_secs(15);

/// Upper bound for stopping the observability server in teardown step 5.
const OBS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Report entry for an observability shutdown that misses its bound.
const ERR_OBS_SHUTDOWN: &str = "observability shutdown did not complete: the metrics listener or \
                                 the poller may still hold the database";

/// Report entry for a metrics listener that missed its own join bound: it may
/// still be serving a scrape and holding the database.
const ERR_OBS_LISTENER_DRAINING: &str =
    "observability shutdown did not complete: the metrics listener is still draining";

/// Report entry for a poller that missed its own join bound: it may still be
/// inside a tick and holding the database.
const ERR_OBS_POLLER_RUNNING: &str =
    "observability shutdown did not complete: the poller is still running";

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
    /// for each `kind: unix` listener (filesystem path / pipe name), `None`
    /// for all others.
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
    /// rate-limiter buckets. Awaited on shutdown so the fjall `server_meta`
    /// store lock is released cleanly before a same-data-dir restart can succeed.
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
    /// fjall stores. Do NOT explicitly unlock — drop is the release.
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

/// One-shot marker for the post-deadline fallback join in `ShutdownOwner::drain`.
#[cfg(test)]
static FALLBACK_DRAIN_JOIN_PROBE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

#[cfg(test)]
pub(crate) struct FallbackDrainJoinProbeGuard;

#[cfg(test)]
impl Drop for FallbackDrainJoinProbeGuard {
    fn drop(&mut self) {
        *FALLBACK_DRAIN_JOIN_PROBE
            .lock()
            .expect("fallback drain join probe") = None;
    }
}

#[cfg(test)]
pub(crate) fn arm_fallback_drain_join_probe(
    probe: impl FnOnce() + Send + 'static,
) -> FallbackDrainJoinProbeGuard {
    *FALLBACK_DRAIN_JOIN_PROBE
        .lock()
        .expect("fallback drain join probe") = Some(Box::new(probe));
    FallbackDrainJoinProbeGuard
}

#[cfg(test)]
fn fire_fallback_drain_join_probe() {
    let probe = FALLBACK_DRAIN_JOIN_PROBE
        .lock()
        .expect("fallback drain join probe")
        .take();
    if let Some(probe) = probe {
        probe();
    }
}

/// Mark entry into the post-expiry owner drain (test-only).
#[cfg(test)]
pub(crate) fn mark_fallback_drain_join() {
    fire_fallback_drain_join_probe();
}

/// One-shot probe fired immediately BEFORE the ordered shutdown's REAL
/// repository flush call (`ShamirDb::flush_all_report`) — never at the point
/// the flush would have started, so a SKIPPED flush leaves the probe unfired
/// and cannot look like an entered one. A shutdown test compares its order with
/// [`arm_scheduled_join_probe`]: the scheduled worker and its in-flight run are
/// joined BEFORE the flush, so the join marker must fire first (mutant: the
/// worker awaited after the flush).
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

/// Bounds WAITs by the earlier of stage start + cap and outer deadline minus reserve; synchronous work is not interrupted, and timed-out joins may continue. `early_stage_bound` uses min(stage start + cap, early cutoff).
fn stage_bound(
    deadline_at: Option<tokio::time::Instant>,
    cap: Duration,
    reserve_after: Duration,
) -> (tokio::time::Instant, Duration) {
    let now = tokio::time::Instant::now();
    let own = now + cap;
    match deadline_at {
        Some(at) => {
            let usable_until = at.checked_sub(reserve_after).unwrap_or(now);
            let until = own.min(usable_until);
            (until, until.saturating_duration_since(now))
        }
        None => (own, cap),
    }
}

fn early_stage_bound(
    early_cutoff: Option<tokio::time::Instant>,
    cap: Duration,
) -> (tokio::time::Instant, Duration) {
    let now = tokio::time::Instant::now();
    let stage_cap = now + cap;
    match early_cutoff {
        Some(cutoff) => {
            let until = stage_cap.min(cutoff);
            (until, until.saturating_duration_since(now))
        }
        None => (stage_cap, cap),
    }
}

fn report_unsubmitted_release_probe(report: &mut ShutdownReport, reason: &str) {
    report.add_error(format!(
        "store release could not be verified: no probe result was obtained; no probe was started / {reason}; \
         whether every store released its directory lock is UNKNOWN; an immediate relaunch of the same \
         data directory can fail with `Locked`"
    ));
}

#[cfg(test)]
pub(super) fn report_unsubmitted_release_probe_for_test(report: &mut ShutdownReport, reason: &str) {
    report_unsubmitted_release_probe(report, reason);
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

    /// Stop accepting connections, then drain the scheduler and audit log.
    /// Returns repository checkpoint failures; ignored reports preserve caller behaviour.
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
    /// `runtime::serve` extracts this BEFORE wrapping shutdown in its deadline,
    /// so expiry cannot drop a running native backup thread or release the data directory.
    /// Callers retaining the handle until consumption (`shutdown`,
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
    ///
    /// The outer-deadline-less entry: the token GC and replication joins
    /// ([`CONN_DRAIN_TIMEOUT`]), the meta-snapshot join
    /// ([`META_SNAPSHOT_JOIN_TIMEOUT`]) and the store-release probe
    /// ([`STORE_RELEASE_TIMEOUT`]) spend their own constants. The scheduled
    /// worker join stays unbounded here by design (cleanup and lock ordering;
    /// tracked as SHUTDOWN-ORACLES), so a starved pool can still stall it.
    /// Callers that wrap this future in a deadline
    /// use [`Self::shutdown_with_budget`] instead, so the late stages can
    /// report what they could not settle BEFORE that deadline erases them.
    pub(crate) async fn shutdown_with_report(
        self,
        report: &mut ShutdownReport,
        owner: &mut ShutdownOwner,
    ) {
        self.shutdown_with_budget(report, owner, None).await;
    }

    /// The teardown and the instant the caller's outer deadline expires.
    ///
    /// `deadline_at` is the instant bounding the caller's timeout, computed
    /// before it starts. The early cutoff is fixed at `outer_deadline -
    /// LATE_RESERVE`; early waits share it, while late stages reserve metadata,
    /// release-probe, and report time.
    ///
    /// cancel-safe: NO — same as [`Self::shutdown_with_report`].
    pub(crate) async fn shutdown_with_budget(
        mut self,
        report: &mut ShutdownReport,
        owner: &mut ShutdownOwner,
        deadline_at: Option<tokio::time::Instant>,
    ) {
        // 1. Tell all accept loops and refusal-audit producers to stop.
        // CancellationToken::cancel() is persistent: every existing waiter
        // resolves on its next poll, and new callers observe cancellation.
        self.cancel_refusal_audit.close();
        self.shutdown_token.cancel();
        // 2. Wait for accept loops to finish, then wait for tracked accepted
        // tasks to quiesce before any repository flush.
        let early_cutoff = deadline_at.map(|outer| {
            outer
                .checked_sub(LATE_RESERVE)
                .unwrap_or_else(tokio::time::Instant::now)
        });
        let (listener_deadline, listener_budget) =
            early_stage_bound(early_cutoff, CONN_DRAIN_TIMEOUT);
        // One fixed early cutoff shared by each bounded wait.
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
                    let joined = tokio::time::timeout_at(
                        early_cutoff
                            .unwrap_or_else(|| tokio::time::Instant::now() + CONN_ABORT_TIMEOUT),
                        &mut task,
                    )
                    .await
                    .is_ok();
                    report.add_error(format!(
                        "HTTP listener drain exceeded {listener_budget:?}; aborted"
                    ));
                    quiesced &= joined;
                }
            }
        }
        // One drain budget for listeners and accepted connections together, so
        // the stage stays inside the runtime shutdown deadline.
        let cutoff =
            early_cutoff.unwrap_or_else(|| tokio::time::Instant::now() + CONN_DRAIN_TIMEOUT);
        let remaining = cutoff.saturating_duration_since(tokio::time::Instant::now());
        let abort_remaining = cutoff
            .saturating_duration_since(tokio::time::Instant::now())
            .min(CONN_ABORT_TIMEOUT);
        let drain = drain_connections(&self.conn_tasks, remaining, abort_remaining, report);
        match tokio::time::timeout_at(cutoff, drain).await {
            Ok(drained) => quiesced &= drained,
            Err(_) => {
                // The drain future was dropped before it could signal the
                // forced abort: signal it here (idempotent).
                self.conn_tasks.abort_all();
                report.add_error(
                    "connection drain and abort exceeded shared early shutdown cutoff".to_string(),
                );
                quiesced = false;
            }
        }
        // Gates stop after transport and connection tasks drain, so the final
        // audit flush includes their notices.
        for gate in &self.http_gates {
            if early_cutoff.is_some() {
                let (gate_deadline, budget) = early_stage_bound(early_cutoff, CONN_DRAIN_TIMEOUT);
                if tokio::time::timeout_at(gate_deadline, gate.shutdown())
                    .await
                    .is_err()
                {
                    report.add_error(format!("HTTP gate shutdown exceeded {budget:?}"));
                }
            } else {
                gate.shutdown().await;
            }
        }
        drop(self.http_gates);
        // Bounded with or without an outer deadline: a GC pass is a blocking job.
        tracing::debug!(stage = "token_gc", "shutdown stage");
        let (gc_deadline, gc_budget) = early_stage_bound(early_cutoff, CONN_DRAIN_TIMEOUT);
        if tokio::time::timeout_at(gc_deadline, self.token_service.shutdown_gc())
            .await
            .is_err()
        {
            report.add_error(format!("token GC shutdown exceeded {gc_budget:?}"));
        }
        // 2b. Stop the follower-replication supervisor: latch the stop admission
        //     and signal every follower first (no lifecycle or registry lock, no
        //     await; delivering the signal is synchronous and is not what the cap
        //     below bounds), then join the followers and the reconcile-tick task
        //     (already told to stop by the root `shutdown_token.cancel()` above).
        //     Done after the accept loops stop and before flush. The asynchronous
        //     join is bounded with or without an outer deadline; an expiry means
        //     follower quiescence was not established (they were already signalled
        //     and exit at their next cancellation checkpoint), so the repository
        //     flush is skipped.
        tracing::debug!(stage = "replication", "shutdown stage");
        self.repl_supervisor.begin_stop();
        let (repl_deadline, repl_budget) = early_stage_bound(early_cutoff, CONN_DRAIN_TIMEOUT);
        if tokio::time::timeout_at(repl_deadline, async {
            self.repl_supervisor.stop_all().await;
            if let Some(task) = &mut self.repl_supervisor_task {
                let _ = (&mut task.handle).await;
            }
        })
        .await
        .is_err()
        {
            report.add_error(format!(
                "replication supervisor shutdown exceeded {repl_budget:?}"
            ));
            quiesced = false;
        }
        // 3. Drain the refusal-audit queue and the audit chain under ONE
        //    shared deadline: the scheduler is signalled before it is joined,
        //    the appender's final drain + anchor write runs on a blocking
        //    thread, and a stalled checkpoint or flusher marks the report
        //    unclean instead of stalling the shutdown future. Teardown
        //    continues to the repository flush either way; a timeout does not
        //    cancel an in-flight fsync.
        if early_cutoff.is_some() {
            // Stop admitting new refusal audits and signal the scheduler. The
            // appender is fenced by `teardown_audit` AFTER it drained the work
            // already accepted (a stop flag now would reject those appends).
            self.cancel_refusal_audit.close();
            self.scheduler.signal_shutdown();
        }
        let audit = teardown_audit(
            AuditTeardown {
                cancel_refusal_audit: &self.cancel_refusal_audit,
                appender: &self.audit_appender,
                scheduler: self.scheduler,
                #[cfg(test)]
                drain_probe: self.refusal_drain_pending,
            },
            report,
        );
        if deadline_at.is_some() {
            let (audit_deadline, audit_budget) = early_stage_bound(
                early_cutoff,
                crate::server::server_launcher::audit_teardown::AUDIT_TEARDOWN_DEADLINE,
            );
            if tokio::time::timeout_at(audit_deadline, audit)
                .await
                .is_err()
            {
                // Timeout abandons this wait; teardown work may continue. Its
                // blocking probe's abort only stops a not-yet-started job; a
                // filesystem operation already running may outlive the verdict.
                self.audit_appender.request_flusher_stop();
                report.add_error(format!(
                    "audit teardown exceeded shutdown budget {audit_budget:?}"
                ));
            }
        } else {
            audit.await;
        }
        // Signal the observability server to stop BEFORE the wait a deadline can
        // abandon; the wait itself is bounded and reported in step 5. The
        // handle's private cancel-on-drop field cancels the same token, so an
        // expiry that drops this future cannot leave a poller and a router
        // holding the database; the full await stays in step 5.
        if let Some(obs) = &self.observability {
            obs.cancel();
        }
        tracing::debug!(stage = "schedule_join", "shutdown stage");
        // Do not abort: an in-flight copy or retention operation must finish
        // cleanup. This is the ORIGINAL position — before the repository flush
        // — and it borrows the owner, so a deadline can abandon this teardown
        // without abandoning the worker (`ShutdownOwner::drain` then awaits it
        // after the deadline).
        if deadline_at.is_some() {
            let (schedule_deadline, schedule_budget) =
                early_stage_bound(early_cutoff, REPO_FLUSH_BUDGET);
            #[cfg(test)]
            fire_scheduled_join_probe();
            if tokio::time::timeout_at(schedule_deadline, owner.join_schedule(report))
                .await
                .is_err()
            {
                report.add_error(format!(
                    "backup schedule join exceeded shutdown budget {schedule_budget:?}"
                ));
                // The repository flush must remain after the scheduled run. Keep the
                // owner intact for the post-deadline join and do not flush ahead of it.
                quiesced = false;
            }
        } else {
            // Keep the probe at the actual unbounded join too: backup-schedule
            // tests call ServerHandle::shutdown(), which has no outer deadline.
            #[cfg(test)]
            fire_scheduled_join_probe();
            owner.join_schedule(report).await;
        }
        // 4. Flush all repo MemBuffers to durable backing so buffered
        //    commits in the last ~500 ms window are not lost on graceful
        //    stop. Must happen AFTER accept loops have stopped (no new
        //    writes can arrive) and BEFORE store locks are released.
        if !quiesced {
            report.add_error(
                "repository flush skipped: connection tasks remain, the scheduled backup owner is still running or replication followers may still apply"
                    .to_string(),
            );
            tracing::error!(
                live = self.conn_tasks.live(),
                "shutdown: repository flush skipped because shutdown ordering is not yet safe",
            );
        } else {
            // The probe belongs to the REAL flush call, not to this point: a
            // skipped flush (unclean connection drain) must not look like an
            // entered one to a shutdown test.
            #[cfg(test)]
            fire_repo_flush_probe();
            let (flush_deadline, flush_budget) = early_stage_bound(early_cutoff, REPO_FLUSH_BUDGET);
            if tokio::time::timeout_at(
                flush_deadline,
                self.shamir.flush_all_report(flush_budget, report),
            )
            .await
            .is_err()
            {
                report.add_error(format!(
                    "repository flush exceeded shutdown budget {flush_budget:?}"
                ));
            }
        }
        // 5. Stop the observability HTTP server (if any). BOUNDED and REPORTED:
        //    the listener may still be draining a scrape and the poller may be
        //    inside its database await, so an expiry must mark the report
        //    unclean instead of dropping the future and detaching both joins.
        //    Dropping the abandoned `obs` still cancels the token (the handle's
        //    private cancel-on-drop field), so the tasks stop on their own at
        //    their next cancellation point.
        if let Some(obs) = self.observability {
            let (obs_deadline, _) = early_stage_bound(early_cutoff, OBS_SHUTDOWN_TIMEOUT);
            match tokio::time::timeout_at(obs_deadline, obs.shutdown()).await {
                // The call returned: name exactly the side(s) that missed their
                // own join bound. A generic message here would blame a side
                // that did stop and hand the operator the wrong subsystem.
                Ok(outcome) => {
                    if !outcome.listener_joined {
                        tracing::warn!(
                            timeout = ?OBS_SHUTDOWN_TIMEOUT,
                            "shutdown: the observability metrics listener is still draining",
                        );
                        report.add_error(ERR_OBS_LISTENER_DRAINING.to_string());
                    }
                    if !outcome.poller_joined {
                        tracing::warn!(
                            timeout = ?OBS_SHUTDOWN_TIMEOUT,
                            "shutdown: the observability poller is still running",
                        );
                        report.add_error(ERR_OBS_POLLER_RUNNING.to_string());
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        timeout = ?OBS_SHUTDOWN_TIMEOUT,
                        "shutdown: observability server did not finish stopping",
                    );
                    report.add_error(ERR_OBS_SHUTDOWN.to_string());
                }
            }
        }
        // 6. Join meta-snapshot task; its final write may wait on the blocking pool.
        //
        //    BOUNDED and REPORTED: that final write is a synchronous store
        //    write submitted to the blocking pool, so a starved pool keeps the
        //    join pending for as long as the pool is starved. An unbounded
        //    await here would let an outer expiry replace this stage's own
        //    verdict with the generic deadline message, so the wait spends
        //    only what is left of the shutdown budget, capped by
        //    META_SNAPSHOT_JOIN_TIMEOUT. With no outer deadline, the same cap
        //    applies without a reserve. Join errors stay ignored exactly as
        //    before: only an EXPIRY means the store may still be locked.
        if let Some(snap) = self.meta_snapshot_task {
            let reserve_after = if deadline_at.is_some() {
                Duration::from_millis(500)
            } else {
                Duration::ZERO
            };
            let (bound_at, bound) =
                stage_bound(deadline_at, META_SNAPSHOT_JOIN_TIMEOUT, reserve_after);
            if tokio::time::timeout_at(bound_at, snap.handle)
                .await
                .is_err()
            {
                tracing::warn!(
                    ?bound,
                    "shutdown: the meta snapshot task did not stop in time",
                );
                report.add_error(format!(
                    "{ERR_META_SNAPSHOT_UNSETTLED} {bound:?}: the periodic metadata snapshot \
                     task has not dropped its `server_meta` store handle, so the `server_meta` \
                     store may still be locked and an immediate reboot on the same data \
                     directory can fail"
                ));
            }
        }
        // 7. Join the interactive-tx reaper; dropping it aborts lingering txs.
        tracing::debug!(stage = "reapers", "shutdown stage");
        if let Some(reaper) = self.interactive_tx_reaper {
            if deadline_at.is_some() {
                let (reaper_deadline, budget) = early_stage_bound(early_cutoff, CONN_DRAIN_TIMEOUT);
                if tokio::time::timeout_at(reaper_deadline, reaper.handle)
                    .await
                    .is_err()
                {
                    report.add_error(format!(
                        "interactive transaction reaper join exceeded {budget:?}"
                    ));
                }
            } else {
                let _ = reaper.handle.await;
            }
        }
        // 8. Join cursor reaper so its Arc drops and pinned snapshots unpin.
        if let Some(reaper) = self.cursor_reaper {
            if deadline_at.is_some() {
                let (reaper_deadline, budget) = early_stage_bound(early_cutoff, CONN_DRAIN_TIMEOUT);
                if tokio::time::timeout_at(reaper_deadline, reaper.handle)
                    .await
                    .is_err()
                {
                    report.add_error(format!("cursor reaper join exceeded {budget:?}"));
                }
            } else {
                let _ = reaper.handle.await;
            }
        }
        // 9. Drop this handle's store references, then probe directory-lock release.
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
        // the store, while an unknown release state identifies whether the
        // blocking job missed its deadline, discovery was incomplete, or a
        // probe failed. This stage uses the one absolute budget, capped by
        // STORE_RELEASE_TIMEOUT and with report slack reserved before expiry.
        // Keep 250 ms before the absolute deadline for recording this verdict
        // and returning the report; do not let the release wait consume it all.
        let (release_deadline, release_budget) = match deadline_at {
            Some(outer) => stage_bound(
                Some(outer),
                STORE_RELEASE_TIMEOUT,
                Duration::from_millis(250),
            ),
            None => stage_bound(None, STORE_RELEASE_TIMEOUT, Duration::ZERO),
        };
        if release_budget.is_zero() {
            tracing::warn!("shutdown: no budget remained to start the store-release probe");
            report_unsubmitted_release_probe(report, "no shutdown budget remained");
        } else {
            match release_stores_until(&data_dir, release_deadline).await {
                ReleaseOutcome::Released => {}
                ReleaseOutcome::StillLocked(still_locked) => {
                    let stores = store_names(&still_locked);
                    tracing::warn!(
                        ?still_locked,
                        timeout = ?release_budget,
                        "shutdown: stores still held after the release timeout (last observed held)",
                    );
                    report.add_error(format!(
                        "stores still held after the release timeout {release_budget:?}: \
                         {stores} — the last completed probe observed these stores held; an immediate \
                         relaunch of the same data directory can fail with `Locked`"
                    ));
                }
                ReleaseOutcome::Unverified(UnverifiedReason::BudgetExpired) => {
                    // UNKNOWN if no usable observation exists. A timed-out
                    // retry after a completed held observation is StillLocked;
                    // abort stops only a queued probe, not a running filesystem call.
                    tracing::warn!("shutdown: release probe deadline expired before submission");
                    report_unsubmitted_release_probe(
                        report,
                        "the deadline expired before submission",
                    );
                }
                ReleaseOutcome::Unverified(reason) => {
                    let (summary, detail) = match reason {
                        UnverifiedReason::PoolStarved => (
                            "the store-release blocking job did not complete before its deadline",
                            "the blocking job may still be queued or running; if running, its filesystem call may continue after this UNKNOWN verdict",
                        ),
                        UnverifiedReason::DiscoveryGap => (
                            "store discovery was incomplete",
                            "the store set could not be fully discovered, or a discovered marker could no longer be verified",
                        ),
                        UnverifiedReason::ProbeError => (
                            "the store-release blocking job ended without a result",
                            "discovery or a lock probe did not complete successfully",
                        ),
                        UnverifiedReason::BudgetExpired => unreachable!("handled above"),
                    };
                    tracing::warn!(
                        ?reason,
                        timeout = ?release_budget,
                        "shutdown: store release could not be verified in time",
                    );
                    report.add_error(format!(
                        "store release could not be verified within {release_budget:?}: {summary}; \
                         {detail}, so whether every store released its directory lock is UNKNOWN; \
                         an immediate relaunch of the same data directory can fail with `Locked`"
                    ));
                }
            }
        }
    }

    /// Returns the first bound TCP+TLS-exporter address for connection tests.
    pub fn first_tls_exporter_addr(&self) -> Option<SocketAddr> {
        self.bound_addrs.iter().filter_map(|a| *a).next()
    }

    /// First bound local-IPC path; see [`Self::first_tls_exporter_addr`].
    pub fn first_ipc_path(&self) -> Option<&str> {
        self.bound_ipc_paths.iter().find_map(|p| p.as_deref())
    }

    /// Whether the experimental online storage-migration API
    /// (`StartMigration`/`CommitMigration`/`RollbackMigration`/`MigrationStatus`)
    /// is enabled on this server. `true` only when
    /// `security.enable_experimental_migration_api` was set at boot (default `false`).
    /// Primarily exposed to test that `server_launcher/launch.rs` wired the toggle
    /// read by every `StartMigration` handler.
    pub fn experimental_migration_enabled(&self) -> bool {
        self.shamir.experimental_migration_enabled()
    }
}
