//! Lifecycle owner: repository-owned structured worker lifecycle
//! (S5c-X PR6b-2b, supervisor-owned-join-handle design).
//!
//! Three objects make up the foundation: this owner, the worker execution
//! state ([`crate::tx::cold_reconcile::lifecycle::WorkerExecutionState`]),
//! and the job envelope. The lifecycle is `Open -> Closing -> Joined`;
//! quarantine is unconditional (the parked-guard count is NEVER consulted),
//! and `shutdown` enforces ONE absolute deadline that covers worker
//! shutdown + commit quiescence + flush + checkpoint, because the worker
//! performs all of those on its exit path. Full contract:
//! `docs/guide-docs/follower-journal-durability.md`.
//!
//! Ownership model: the owner does NOT hold the worker's join handle. The
//! handle and a lease `Arc` are owned by a spawned SUPERVISOR task for the
//! rest of the lifecycle: the supervisor joins the worker, releases the
//! lease (the last `Arc<L>`), publishes the terminal outcome into a
//! `watch` channel (first-write-wins), and only then sets
//! `supervisor_joined`. Dropping the owner therefore cannot detach the
//! worker task, and the physical lease lifetime ends strictly after join.
//!
//! Contention model: the `control` std-Mutex is a lifecycle control-plane
//! critical section — low frequency (setup/teardown only, never a hot
//! path), never held across an `.await`, and the mpsc `try_send` inside it
//! is non-blocking. `CoreState` is lock-free (atomics + a `watch` sender)
//! and `Arc`-shared with the worker and supervisor.
//!
//! Lock order: `control` only; no other lock interacts with it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::interlock::PubTermInterlock;
use super::job::{JobSubmitError, LifecycleJob};
use super::worker::{WorkerExecutionState, WorkerLease};

/// Default capacity of the job channel between owner and worker.
/// (Kept as a constant only; the dead `channel_bound()` accessor and field
/// were removed in the supervisor redesign.)
const DEFAULT_CHANNEL_BOUND: usize = 16;

/// Terminal disposition of the worker task. `Failed` covers a panicked or
/// aborted worker; the cause is stringified into it.
#[derive(Debug, Clone, thiserror::Error)]
pub enum WorkerTerminal {
    /// The worker task ran to completion.
    #[error("worker completed")]
    Completed,
    /// The worker task panicked (or was cancelled); payload stringified.
    #[error("worker failed: {0}")]
    Failed(String),
}

/// Shared owner/supervisor/worker state — all `Arc`-shared; atomics plus a
/// `watch` sender only (lock-free; `Send + Sync` by construction: atomics
/// and `tokio::sync::watch::Sender<Option<WorkerTerminal>>` are both
/// `Send + Sync` for `T: Send + Sync`).
pub(crate) struct CoreState {
    interlock: PubTermInterlock,
    /// Sticky quarantine flag visible to repo reopen logic.
    quarantined: AtomicBool,
    /// Sticky flag: repository reopen is required before further use.
    reopen_required: AtomicBool,
    /// Sticky flag set by the supervisor strictly AFTER joining the worker
    /// and dropping the lease, BEFORE the terminal becomes observable —
    /// tests use it to prove lease-release ordering.
    supervisor_joined: AtomicBool,
    /// Terminal publication channel (first-write-wins; `None` until the
    /// supervisor publishes the outcome).
    terminal_tx: tokio::sync::watch::Sender<Option<WorkerTerminal>>,
}

impl CoreState {
    /// Handle to the publication/termination interlock (worker side).
    pub(crate) fn interlock(&self) -> &PubTermInterlock {
        &self.interlock
    }

    /// UNCONDITIONAL quarantine: sticky `quarantined` + `reopen_required`
    /// (both Release), then `interlock.terminate()`. The parked-guard count
    /// is NEVER consulted. Timeout and panic BOTH route here.
    fn quarantine(&self, reason: &str) {
        log::warn!("worker lifecycle quarantine: {reason}");
        self.quarantined.store(true, Ordering::Release);
        self.reopen_required.store(true, Ordering::Release);
        self.interlock.terminate();
    }

    /// First-write-wins terminal publication (supervisor/worker race safe):
    /// only the FIRST `set_terminal` writes; later calls are no-ops. This
    /// is what keeps a caught worker panic (`Failed`, published inside the
    /// worker boundary) from being overwritten by the supervisor's
    /// `Completed`.
    fn set_terminal(&self, terminal: WorkerTerminal) {
        let _ = self.terminal_tx.send_if_modified(|slot| {
            if slot.is_none() {
                *slot = Some(terminal);
                true
            } else {
                false
            }
        });
    }
}

/// Error returned by [`RepoWorkerLifecycle::install_worker`].
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The lifecycle is closing/closed/quarantined, or a previous
    /// close-before-initialize permanently forbade spawning.
    #[error("worker install rejected: lifecycle closed or spawn forbidden")]
    ClosedOrForbidden,
    /// A worker was already installed for this lifecycle.
    #[error("worker install rejected: worker already installed")]
    AlreadyInstalled,
    /// Kept for install-API symmetry: the install was rejected WITHOUT
    /// spawning, so nothing is untracked.
    #[error("worker install rejected: install slot busy")]
    #[allow(dead_code)] // kept per design contract; not constructible in the supervisor design
    SlotBusy,
}

/// Error returned by [`RepoWorkerLifecycle::shutdown`].
#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    /// The absolute shutdown deadline elapsed before the worker exited.
    /// Quarantine was applied; the supervisor retains the handle and the
    /// lease, and the terminal becomes observable if/when the worker exits.
    #[error("worker shutdown deadline elapsed")]
    TimedOut,
}

/// Lifecycle state machine: `Open -> Closing -> Joined`.
#[derive(Debug)]
enum LifecycleState {
    /// Admissions open, worker may be installed.
    Open,
    /// Admission closed (sender dropped); buffered jobs still drain FIFO.
    Closing,
    /// The worker was joined and its terminal outcome is known.
    Joined(WorkerTerminal),
}

/// The ONE control critical section: state + sender + spawn bookkeeping +
/// the pending receiver, all mutated together and never across an await.
struct Control<J, R> {
    state: LifecycleState,
    sender: Option<mpsc::Sender<LifecycleJob<J, R>>>,
    pending_rx: Option<mpsc::Receiver<LifecycleJob<J, R>>>,
    spawned: bool,
    spawn_forbidden: bool,
}

/// Repository-owned worker lifecycle owner. Generic over the job payload
/// `J` and reply `R` so the receipt pipeline plugs in later; the worker
/// stays INACTIVE in production until wired.
///
/// Drop contract: dropping the last public owner closes admission
/// synchronously (`Drop` calls `close`) — no abort, no panic. The owner
/// does NOT hold the worker's join handle (the supervisor does), so
/// dropping the owner cannot detach the task; physical worker resources
/// (the lease) end only after the supervisor joins the worker.
pub struct RepoWorkerLifecycle<J: Send, R: Send> {
    control: Mutex<Control<J, R>>,
    core: Arc<CoreState>,
    /// This owner's own receiver on the terminal channel (cloned per
    /// `shutdown` call; the cached-terminal read comes from here too).
    terminal: tokio::sync::watch::Receiver<Option<WorkerTerminal>>,
    /// Monotonic generation handed to each installed worker.
    generation: AtomicU64,
}

impl<J: Send, R: Send> Default for RepoWorkerLifecycle<J, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<J: Send, R: Send> RepoWorkerLifecycle<J, R> {
    /// Creates an `Open` lifecycle with a pre-connected job channel but no
    /// worker. Spawning is a separate, explicit step (`install_worker`).
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(DEFAULT_CHANNEL_BOUND);
        let (terminal_tx, terminal_rx) = tokio::sync::watch::channel(None);
        Self {
            control: Mutex::new(Control {
                state: LifecycleState::Open,
                sender: Some(tx),
                pending_rx: Some(rx),
                spawned: false,
                spawn_forbidden: false,
            }),
            core: Arc::new(CoreState {
                interlock: PubTermInterlock::new(),
                quarantined: AtomicBool::new(false),
                reopen_required: AtomicBool::new(false),
                supervisor_joined: AtomicBool::new(false),
                terminal_tx,
            }),
            terminal: terminal_rx,
            generation: AtomicU64::new(0),
        }
    }

    /// Shared publication/termination interlock: the owner and the worker
    /// pipeline's publication path use the SAME object.
    pub fn interlock(&self) -> &PubTermInterlock {
        &self.core.interlock
    }

    /// Sticky: quarantine was applied to this lifecycle epoch.
    pub fn quarantined(&self) -> bool {
        self.core.quarantined.load(Ordering::Acquire)
    }

    /// Sticky: the repository must reopen (recovering WAL-unsettled
    /// versions) before further use of this lifecycle epoch.
    pub fn reopen_required(&self) -> bool {
        self.core.reopen_required.load(Ordering::Acquire)
    }

    /// Sticky: the supervisor has joined the worker and dropped its lease
    /// `Arc`. Set strictly BEFORE the terminal becomes observable via
    /// `shutdown`.
    pub fn supervisor_joined(&self) -> bool {
        self.core.supervisor_joined.load(Ordering::Acquire)
    }

    /// Monotonic generation counter handed to each installed worker.
    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel)
    }

    /// `true` while admission is open (state == `Open`).
    pub fn admission_open(&self) -> bool {
        matches!(
            self.control.lock().unwrap_or_else(|e| e.into_inner()).state,
            LifecycleState::Open
        )
    }

    /// Installs the worker: synchronously checks admission, takes the
    /// pending receiver, spawns the worker task with a panic boundary, and
    /// spawns the SUPERVISOR that owns the join handle and a lease `Arc`
    /// for the rest of the lifecycle — all inside ONE synchronous critical
    /// section with no cancellation point (`tokio::spawn` is
    /// non-blocking; the `control` std-Mutex is never held across an
    /// await). On any rejection the tasks are NOT spawned: a
    /// close-before-initialize can never leave a spawned-but-untracked
    /// task, and a close concurrent with this call PERMANENTLY sets
    /// `spawn_forbidden`.
    ///
    /// The supervisor keeps the join handle and the lease until the worker
    /// is joined; the owner does NOT hold the handle, so dropping the
    /// owner cannot detach the task; the physical lease lifetime ends
    /// after join (the supervisor drops the last `Arc<L>` before
    /// publishing the terminal outcome).
    pub fn install_worker<L, F, Fut>(&self, lease: L, handler: F) -> Result<(), InstallError>
    where
        L: WorkerLease,
        F: FnOnce(WorkerExecutionState<J, R, L>) -> Fut + Send + 'static,
        // `tokio::spawn` requires the spawned future's captured state to
        // be `'static`, which propagates `J: 'static` / `R: 'static`.
        J: 'static,
        R: 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        if control.spawned {
            return Err(InstallError::AlreadyInstalled);
        }
        if !matches!(control.state, LifecycleState::Open) || control.spawn_forbidden {
            if !matches!(control.state, LifecycleState::Open) {
                control.spawn_forbidden = true;
            }
            return Err(InstallError::ClosedOrForbidden);
        }
        let receiver = control
            .pending_rx
            .take()
            .expect("Open lifecycle always holds the pending receiver");
        let generation = self.next_generation();
        let lease: Arc<L> = Arc::new(lease);
        let state = WorkerExecutionState::new(
            receiver,
            Arc::clone(&lease),
            generation,
            Arc::clone(&self.core),
        );
        // Panic boundary: `Box::pin` so the future (and the guards captured
        // inside it) can be dropped EXPLICITLY after quarantine, never by
        // unwind. `catch_unwind` keeps the (already-pinned) future alive in
        // `caught` when it panics, so its guards are NOT dropped by the
        // unwind itself.
        let core_w = Arc::clone(&self.core);
        let lease_w = Arc::clone(&lease);
        let inner = async move { handler(state).await };
        let worker_task = async move {
            let mut caught = Box::pin(futures::FutureExt::catch_unwind(
                std::panic::AssertUnwindSafe(inner),
            ));
            if caught.as_mut().await.is_err() {
                // Panic caught INSIDE the worker execution: quarantine, run
                // the FAIL-CLOSED lease cleanup, and publish the terminal —
                // all BEFORE the future (and every guard it still owns) is
                // dropped below. Guard destruction happens in
                // `drop(caught)`, strictly after quarantine.
                core_w.quarantine("worker panicked");
                lease_w.on_quarantine();
                core_w.set_terminal(WorkerTerminal::Failed("worker panicked".to_string()));
            }
            drop(caught); // guard destruction happens HERE, strictly after quarantine
        };
        let worker_handle = tokio::spawn(worker_task);
        // The supervisor OWNS the join handle and a lease Arc for the rest
        // of the lifecycle — this is what makes Drop-of-owner unable to
        // detach the worker.
        let _supervisor = spawn_supervisor(Arc::clone(&self.core), worker_handle, lease);
        control.spawned = true;
        Ok(())
    }

    /// Synchronously submits a job. The same critical section holds state,
    /// the spawned flag, AND sender, so a close or a never-installed worker
    /// concurrent with this call serializes: a successful `try_send` while
    /// state == `Open` AND a worker exists IS the acceptance linearization
    /// point.
    ///
    /// Policy: NO work is accepted before a worker exists — the
    /// `spawned` check below makes `NotInitialized` deterministic instead
    /// of relying on `try_send` racing the channel close. (Installing at
    /// construction is impossible: the handler is supplied later, via
    /// `install_worker`.)
    pub fn submit(&self, payload: J) -> Result<tokio::sync::oneshot::Receiver<R>, JobSubmitError> {
        let control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(control.state, LifecycleState::Open) {
            return Err(JobSubmitError::Closing);
        }
        if !control.spawned {
            return Err(JobSubmitError::NotInitialized);
        }
        let sender = control
            .sender
            .as_ref()
            .ok_or(JobSubmitError::NotInitialized)?;
        let (job, rx) = LifecycleJob::new(payload);
        sender.try_send(job).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => JobSubmitError::Full,
            mpsc::error::TrySendError::Closed(_) => JobSubmitError::NotInitialized,
        })?;
        Ok(rx)
    }

    /// Closes admission: transitions `Open -> Closing` and drops the
    /// sender so the worker drains buffered jobs FIFO and then observes
    /// channel end. Idempotent; safe to call from `Drop`.
    pub fn close(&self) {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(control.state, LifecycleState::Open) {
            control.state = LifecycleState::Closing;
            // Dropping the sender closes the channel; the mpsc Receiver
            // keeps yielding buffered jobs after the sender drop.
            control.sender = None;
        }
    }

    /// UNCONDITIONAL quarantine: sticky `quarantined` + `reopen_required`,
    /// `interlock.terminate()`, then `close()` (admission closed). Timeout
    /// and panic BOTH call this; the parked-guard count is NEVER consulted.
    pub fn quarantine(&self, reason: String) {
        self.core.quarantine(&reason);
        self.close();
    }

    /// THE one absolute shutdown deadline (covers worker drain + commit
    /// quiescence + flush + checkpoint, because the worker performs all of
    /// those on its exit path; enforced by callers).
    ///
    /// Contract: closes admission first (idempotent), then waits on the
    /// terminal `watch` channel under the deadline. The join happens
    /// EXACTLY ONCE, in the supervisor (which owns the handle and the
    /// lease); repeated/concurrent callers each `wait_for` on cloned
    /// receivers and all observe the single cached terminal. A timed-out
    /// or cancelled waiter cancels nothing — the close is already applied
    /// and the supervisor keeps ownership of the handle + lease; a timeout
    /// additionally quarantines unconditionally (fail-closed) while the
    /// supervisor retains handle + lease.
    pub async fn shutdown(&self, deadline: Duration) -> Result<WorkerTerminal, ShutdownError> {
        self.close();
        let mut rx = self.terminal.clone();
        if let Some(t) = rx.borrow().clone() {
            // Cached terminal: the supervisor already joined and published;
            // never repoll the worker.
            self.record_joined(t.clone());
            return Ok(t);
        }
        match tokio::time::timeout(deadline, rx.wait_for(|t| t.is_some())).await {
            Ok(Ok(guard)) => {
                let t = guard
                    .as_ref()
                    .cloned()
                    .expect("wait_for predicate guarantees Some");
                self.record_joined(t.clone());
                Ok(t)
            }
            Ok(Err(_recv_closed)) => {
                // Unreachable: the sender lives in `core` (`Arc<CoreState>`,
                // shared with the supervisor), which outlives this call —
                // fail closed anyway.
                self.quarantine("terminal channel closed".to_string());
                Err(ShutdownError::TimedOut)
            }
            Err(_elapsed) => {
                self.quarantine("worker shutdown deadline elapsed".to_string());
                Err(ShutdownError::TimedOut)
            }
        }
    }

    /// Records the `Closing -> Joined(terminal)` transition on this
    /// owner's control state. Called only from `shutdown` success paths
    /// (the supervisor performs the actual join). Poisoning-tolerant: a
    /// panicking holder must not wedge the lifecycle control plane.
    fn record_joined(&self, terminal: WorkerTerminal) {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        control.state = LifecycleState::Joined(terminal);
    }
}

/// Spawns the supervisor task: it awaits the worker's join handle, drops
/// the LAST lease `Arc` strictly AFTER the join and BEFORE the terminal
/// becomes observable, sets `supervisor_joined`, then publishes the
/// terminal outcome (first-write-wins).
fn spawn_supervisor<L: WorkerLease>(
    core: Arc<CoreState>,
    handle: JoinHandle<()>,
    lease: Arc<L>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let join_result = handle.await;
        let terminal = {
            // `lease` (the LAST Arc<L>: the worker task already ended before
            // `handle.await` returned) is dropped at the end of this block —
            // strictly AFTER join, BEFORE the terminal becomes observable.
            match join_result {
                Ok(()) => {
                    // A caught panic already published Failed (first-write-
                    // wins keeps it); Completed only if the worker exited
                    // cleanly.
                    WorkerTerminal::Completed
                }
                Err(join_err) => {
                    // Defensive: the worker boundary catches job panics;
                    // reaching here means the wrapper task itself died
                    // (abort or boundary unwind). Fail closed.
                    core.quarantine("worker task aborted");
                    lease.on_quarantine();
                    WorkerTerminal::Failed(join_err.to_string())
                }
            }
        };
        core.supervisor_joined
            .store(true, std::sync::atomic::Ordering::Release);
        core.set_terminal(terminal);
    })
}

impl<J: Send, R: Send> Drop for RepoWorkerLifecycle<J, R> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Unit-payload alias used by the production-inactive wiring: no job
/// payload, no reply value.
pub type RepoUnitLifecycle = RepoWorkerLifecycle<(), ()>;
