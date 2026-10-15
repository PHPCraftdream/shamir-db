//! Lifecycle owner: repository-owned structured worker lifecycle
//! (S5c-X PR6b-2b).
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
//! Contention model: the `control` std-Mutex is a lifecycle control-plane
//! critical section — low frequency, never held across an await (the only
//! await-held lock is `join_slot` inside `shutdown`), and the mpsc
//! `try_send` is non-blocking. Lock order: `join_slot` BEFORE `control`;
//! `control` never acquires `join_slot`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use super::interlock::PubTermInterlock;
use super::job::{JobSubmitError, LifecycleJob};
use super::worker::{WorkerExecutionState, WorkerLease};

/// Default capacity of the job channel between owner and worker.
const DEFAULT_CHANNEL_BOUND: usize = 16;

/// Terminal disposition of the worker task. `Failed` covers a panicked
/// worker: the `JoinError` payload is stringified into it.
#[derive(Debug, Clone, thiserror::Error)]
pub enum WorkerTerminal {
    /// The worker task ran to completion.
    #[error("worker completed")]
    Completed,
    /// The worker task panicked (or was cancelled); payload stringified.
    #[error("worker failed: {0}")]
    Failed(String),
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
    /// The join slot was contended, which must not happen before a worker
    /// exists (it would mean a shutdown waiter raced installation); the
    /// install was rejected WITHOUT spawning, so nothing is untracked.
    #[error("worker install rejected: join slot busy")]
    SlotBusy,
}

/// Error returned by [`RepoWorkerLifecycle::shutdown`].
#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    /// The absolute shutdown deadline elapsed before the worker exited.
    /// Quarantine was applied; the worker handle stays stored so a later
    /// `shutdown` call can still join it.
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

/// Join slot: the worker handle, taken exactly once, plus the cached
/// terminal so concurrent/late shutdown callers never repoll.
#[derive(Debug, Default)]
struct JoinSlot {
    handle: Option<JoinHandle<()>>,
    terminal: Option<WorkerTerminal>,
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
/// synchronously (`Drop` calls `close`) and signals close — no abort, no
/// panic. The physical worker resources end only after a successful
/// `shutdown` join, because the lease lives inside the worker task.
pub struct RepoWorkerLifecycle<J: Send, R: Send> {
    control: Mutex<Control<J, R>>,
    join_slot: AsyncMutex<JoinSlot>,
    /// Shared publication/termination interlock; the worker execution
    /// state holds a clone via `Arc`.
    interlock: Arc<PubTermInterlock>,
    /// Sticky quarantine flag visible to repo reopen logic.
    quarantined: AtomicBool,
    /// Sticky flag: repository reopen is required before further use.
    reopen_required: AtomicBool,
    next_generation: AtomicU64,
    channel_bound: usize,
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
        Self {
            control: Mutex::new(Control {
                state: LifecycleState::Open,
                sender: Some(tx),
                pending_rx: Some(rx),
                spawned: false,
                spawn_forbidden: false,
            }),
            join_slot: AsyncMutex::new(JoinSlot::default()),
            interlock: Arc::new(PubTermInterlock::new()),
            quarantined: AtomicBool::new(false),
            reopen_required: AtomicBool::new(false),
            next_generation: AtomicU64::new(0),
            channel_bound: DEFAULT_CHANNEL_BOUND,
        }
    }

    /// Shared publication/termination interlock: the owner and the worker
    /// pipeline's publication path use the SAME object.
    pub fn interlock(&self) -> &PubTermInterlock {
        &self.interlock
    }

    /// Sticky: quarantine was applied to this lifecycle epoch.
    pub fn quarantined(&self) -> bool {
        self.quarantined.load(Ordering::Acquire)
    }

    /// Sticky: the repository must reopen (recovering WAL-unsettled
    /// versions) before further use of this lifecycle epoch.
    pub fn reopen_required(&self) -> bool {
        self.reopen_required.load(Ordering::Acquire)
    }

    /// Monotonic generation counter handed to each installed worker.
    pub fn next_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::AcqRel)
    }

    /// Capacity of the job channel (stored for spawn configuration).
    pub fn channel_bound(&self) -> usize {
        self.channel_bound
    }

    /// `true` while admission is open (state == `Open`).
    pub fn admission_open(&self) -> bool {
        matches!(
            self.control.lock().unwrap_or_else(|e| e.into_inner()).state,
            LifecycleState::Open
        )
    }

    /// Installs the worker: synchronously checks admission, takes the
    /// pending receiver, spawns the task, and stores its handle — all
    /// without an intervening await/cancellation point. On any rejection
    /// the task is NOT spawned: close-before-initialize can never leave a
    /// spawned-but-untracked task, and a close concurrent with this call
    /// PERMANENTLY sets `spawn_forbidden`. This method must not be called
    /// while holding a lock that `shutdown` waits on (lock order above).
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
        // Lock order: join_slot before control; both acquired without
        // blocking, so this method has no cancellation point.
        let mut slot = self
            .join_slot
            .try_lock()
            .map_err(|_| InstallError::SlotBusy)?;
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
        let state =
            WorkerExecutionState::new(receiver, lease, generation, Arc::clone(&self.interlock));
        let handle = tokio::spawn(handler(state));
        control.spawned = true;
        slot.handle = Some(handle);
        Ok(())
    }

    /// Synchronously submits a job. The same critical section holds state
    /// AND sender, so a close concurrent with this call serializes: a
    /// successful `try_send` while state == `Open` IS the acceptance
    /// linearization point.
    pub fn submit(&self, payload: J) -> Result<tokio::sync::oneshot::Receiver<R>, JobSubmitError> {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(control.state, LifecycleState::Open) {
            return Err(JobSubmitError::Closing);
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
        log::warn!("worker lifecycle quarantine: {reason}");
        self.quarantined.store(true, Ordering::Release);
        self.reopen_required.store(true, Ordering::Release);
        self.interlock.terminate();
        self.close();
    }

    /// THE one absolute shutdown deadline (covers worker shutdown + commit
    /// quiescence + flush + checkpoint, because the worker performs all of
    /// those on its exit path).
    ///
    /// Contract: closes admission first (idempotent), then joins the
    /// worker under the deadline. Repeated/concurrent calls serialize on
    /// `join_slot`; the winner takes the handle and caches the terminal;
    /// later/concurrent callers observe the cache and return without
    /// repolling. A timed-out or cancelled wait leaves the handle stored
    /// for the next caller (cancelling this future never cancels the
    /// close already applied). A panicked worker quarantines the lifecycle
    /// before `Failed` is returned; a timeout quarantines and returns
    /// [`ShutdownError::TimedOut`].
    pub async fn shutdown(&self, deadline: Duration) -> Result<WorkerTerminal, ShutdownError> {
        self.close();
        let mut slot = self.join_slot.lock().await;
        if let Some(terminal) = slot.terminal.as_ref() {
            return Ok(terminal.clone());
        }
        let Some(handle) = slot.handle.as_mut() else {
            // Never installed: close with no worker — nothing to join.
            let terminal = WorkerTerminal::Completed;
            slot.terminal = Some(terminal.clone());
            self.record_joined(terminal.clone());
            return Ok(terminal);
        };
        match tokio::time::timeout(deadline, handle).await {
            Ok(Ok(())) => {
                slot.handle = None;
                let terminal = WorkerTerminal::Completed;
                slot.terminal = Some(terminal.clone());
                self.record_joined(terminal.clone());
                Ok(terminal)
            }
            Ok(Err(join_err)) => {
                slot.handle = None;
                let message = if join_err.is_panic() {
                    match join_err.into_panic().downcast::<String>() {
                        Ok(payload) => *payload,
                        Err(payload) => match payload.downcast::<&'static str>() {
                            Ok(text) => (*text).to_string(),
                            Err(_) => "worker panicked (opaque payload)".to_string(),
                        },
                    }
                } else {
                    join_err.to_string()
                };
                self.quarantine("worker panicked".to_string());
                let terminal = WorkerTerminal::Failed(message);
                slot.terminal = Some(terminal.clone());
                self.record_joined(terminal.clone());
                Ok(terminal)
            }
            Err(_elapsed) => {
                // Do NOT take the handle — it stays stored for the next
                // caller. The state stays `Closing` until a later
                // successful join records `Joined`.
                self.quarantine("worker shutdown deadline elapsed".to_string());
                Err(ShutdownError::TimedOut)
            }
        }
    }

    /// Records the `Closing -> Joined(terminal)` transition. Callers hold
    /// `join_slot`; `control` is acquired after, per the documented lock
    /// order. Poisoning-tolerant: a panicking holder must not wedge the
    /// lifecycle control plane.
    fn record_joined(&self, terminal: WorkerTerminal) {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        control.state = LifecycleState::Joined(terminal);
    }
}

impl<J: Send, R: Send> Drop for RepoWorkerLifecycle<J, R> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Unit-payload alias used by the production-inactive wiring: no job
/// payload, no reply value.
pub type RepoUnitLifecycle = RepoWorkerLifecycle<(), ()>;
