//! Worker execution state + lease hook (S5c-X PR6b-2b).
//!
//! The worker's execution state carries the job receiver, an `Arc` to the
//! repository resource lease, a generation number, and an `Arc` to the
//! shared owner/supervisor/worker core state (which owns the
//! publication/termination interlock). It holds NO sender and NO
//! lifecycle-owner reference (not even a `Weak`): the worker cannot reach
//! back into the owner, which keeps the ownership graph one-directional.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::interlock::PubTermInterlock;
use super::job::LifecycleJob;
use super::owner::{CoreState, InstallError, RepoWorkerLifecycle};

/// The repository resource lease hook. The receipt pipeline implements
/// `on_quarantine` to run the FAIL-CLOSED unsettled drop for parked
/// version guards on the quarantine cleanup path — never an ordinary
/// abort. Implementations MUST preserve unresolved versions (recoverable
/// via repository reopen, see `shamir-tx` `VersionGuard`
/// `visibility_started_without_wal` / reopen-on-drop semantics) rather
/// than silently resolving them.
pub trait WorkerLease: Send + 'static {
    /// Quarantine cleanup path. Default: no-op.
    fn on_quarantine(&self) {}
}

/// Everything the worker task owns for one lifecycle epoch. No owner
/// reference, no sender (see module doc).
pub struct WorkerExecutionState<J, R, L> {
    receiver: mpsc::Receiver<LifecycleJob<J, R>>,
    /// Shared with the supervisor, which drops the LAST `Arc<L>` strictly
    /// after joining the worker task — the physical lease lifetime ends
    /// after join.
    lease: Arc<L>,
    generation: u64,
    core: Arc<CoreState>,
}

impl<J, R, L> WorkerExecutionState<J, R, L> {
    /// Assembles the execution state (called by the owner's
    /// `install_worker`).
    pub(crate) fn new(
        receiver: mpsc::Receiver<LifecycleJob<J, R>>,
        lease: Arc<L>,
        generation: u64,
        core: Arc<CoreState>,
    ) -> Self {
        Self {
            receiver,
            lease,
            generation,
            core,
        }
    }

    /// Next queued job, or `None` once the channel is closed AND drained.
    pub async fn next_job(&mut self) -> Option<LifecycleJob<J, R>> {
        self.receiver.recv().await
    }

    /// The repository resource lease.
    pub fn lease(&self) -> &L {
        &self.lease
    }

    /// Monotonic generation of this lifecycle epoch.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Quarantine cleanup: runs the lease's FAIL-CLOSED drop. Called on
    /// the worker's exit path when the lifecycle quarantined.
    pub fn quarantine_cleanup(&self) {
        self.lease.on_quarantine();
    }

    /// Handle to the shared publication/termination interlock.
    pub fn interlock(&self) -> &PubTermInterlock {
        self.core.interlock()
    }
}

/// Thin wrapper over [`RepoWorkerLifecycle::install_worker`]: installs a
/// worker task on `owner` with the given lease and job handler. Returns
/// an error (WITHOUT spawning) if admission is closed, a worker is
/// already installed, or the install slot is busy.
pub fn spawn_worker<J, R, L, F, Fut>(
    owner: &Arc<RepoWorkerLifecycle<J, R>>,
    lease: L,
    handler: F,
) -> Result<(), InstallError>
where
    J: Send + 'static,
    R: Send + 'static,
    L: WorkerLease,
    F: FnOnce(WorkerExecutionState<J, R, L>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    owner.install_worker(lease, handler)
}
