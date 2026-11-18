//! Shared admission for a server's logical-export freeze.
use super::guard::MaintenanceGuard;
use futures::future::BoxFuture;
use shamir_engine_table::writer_drain_barrier::{WriterDrainBarrier, WriterDrainGuard};
use shamir_storage::error::{DbError, DbResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// One instance must be shared by ALL server mutation entry points, including
/// identity/ACL administration, replication, DDL and background maintenance.
/// Acquire admission before any repository/table lock, and retain it through
/// the last mutation (including detached commit tails). Nested operations reuse
/// the outer admission; they must not enter again after a freeze is requested.
///
/// This is an admission primitive, not automatic interception of raw engine
/// handles. An embedding that has not enrolled every mutation source MUST NOT
/// advertise online consistent export. Offline callers must exclude background
/// mutation and hold exclusive ownership before using it.
///
/// The thaw channel is [`Notify`] because thaws are broadcast to every queued
/// waiter at once; `notify_waiters` stores no permit, so each waiter must arm
/// its `Notified` future before re-checking the flag (see
/// `wait_for_mutation`).
#[derive(Clone, Default)]
pub struct MaintenanceGate {
    frozen: Arc<AtomicBool>,
    /// Wakes queued `wait_for_mutation` waiters on every thaw.
    /// `Notify::notify_waiters` stores no permit, so correctness relies on
    /// each waiter arming its `Notified` future (enable) BEFORE checking the
    /// flag and re-checking after every wakeup — see `wait_for_mutation`.
    thaw: Arc<Notify>,
    writers: WriterDrainBarrier,
}

impl MaintenanceGate {
    /// Refuse new mutations during maintenance.
    pub fn enter_mutation(&self) -> DbResult<WriterDrainGuard> {
        match self.try_enter() {
            Some(writer) => Ok(writer),
            None => Err(DbError::Conflict(
                "logical export maintenance in progress".into(),
            )),
        }
    }

    /// Single admission step shared by `enter_mutation` and the waiter loop:
    /// bump the drain set, then refuse (`None`) if freeze intent is raised.
    /// Bump before checking the flag so the SeqCst drain protocol catches
    /// every writer that observed the old flag. `None` carries no error:
    /// parked waiters neither allocate nor build a `DbError`; only the public
    /// non-wait `enter_mutation` maps it to [`DbError::Conflict`].
    fn try_enter(&self) -> Option<WriterDrainGuard> {
        let writer = self.writers.enter_writer();
        if self.frozen.load(Ordering::SeqCst) {
            None
        } else {
            Some(writer)
        }
    }

    /// Async admission: wait until the gate is open, then enter as a writer
    /// exactly like [`Self::enter_mutation`].
    ///
    /// The counted writer is acquired only on the final, non-suspending pass
    /// and is never held across the await: a parked waiter that still counted
    /// itself into the drain set would stall an in-progress drain.
    ///
    /// The loop uses the enable-before-check protocol required by
    /// `notify_waiters` (which stores no permit and wakes only futures armed
    /// before the call), so no thaw wakeup can be lost between the flag check
    /// and the suspension:
    ///
    /// 1. fast-path `try_enter` keeps the open-gate hot path off the Notify
    ///    mutex;
    /// 2. arm (`Notified::enable`) BEFORE `try_enter`, so a `notify_waiters`
    ///    landing between the check and the await is observed instead of lost;
    /// 3. re-check the flag after every wakeup (the waker may predate the
    ///    thaw) and replace the spent `Notified` before the next arm — a spent
    ///    future must never be re-polled.
    ///
    /// The guard's `Drop` thaws in order (clear the `frozen` flag FIRST, then
    /// `notify_waiters`), so any waiter this wakeup reaches re-checks an
    /// already-thawed flag.
    ///
    /// # Cancel-safety
    ///
    /// `wait_for_mutation` is cancel-safe. Cancellation at any point after the
    /// fast path drops only the local, not-yet-enabled `Notified` future (a
    /// disabled arm is inert and unregistered by drop); the fast path and the
    /// final `try_enter` run to completion within a single poll and either
    /// return a writer or leave no gate state mutated, so the caller may retry
    /// from scratch.
    pub async fn wait_for_mutation(&self) -> WriterDrainGuard {
        // Fast path: keep the open-gate hot path off the Notify mutex.
        if let Some(writer) = self.try_enter() {
            return writer;
        }
        let notified = self.thaw.notified();
        tokio::pin!(notified);
        loop {
            // Arm BEFORE the flag check: a notify_waiters landing between the
            // check and the await is then observed instead of lost.
            notified.as_mut().enable();
            if let Some(writer) = self.try_enter() {
                return writer;
            }
            notified.as_mut().await;
            // The spent `Notified` must never be re-polled — replace it.
            notified.set(self.thaw.notified());
        }
    }

    /// Freeze, drain, run a bounded export, and release on success, error,
    /// timeout or cancellation. The 600-second budget includes drain time.
    /// The callback borrows the unforgeable guard: it cannot return a reader
    /// that outlives the freeze. No watchdog can thaw a still-running export.
    pub async fn with_freeze<T, F>(&self, export: F) -> DbResult<T>
    where
        F: for<'a> FnOnce(&'a MaintenanceGuard) -> BoxFuture<'a, DbResult<T>>,
    {
        self.with_deadline(Instant::now() + Duration::from_secs(600), export)
            .await
    }

    pub(super) async fn with_deadline<T, F>(&self, deadline: Instant, export: F) -> DbResult<T>
    where
        F: for<'a> FnOnce(&'a MaintenanceGuard) -> BoxFuture<'a, DbResult<T>>,
    {
        self.frozen
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| DbError::Conflict("logical export already in progress".into()))?;
        // Construct before the first await: cancellation while draining releases.
        let guard =
            MaintenanceGuard::new(Arc::clone(&self.frozen), Arc::clone(&self.thaw), deadline);
        tokio::time::timeout_at(deadline, async {
            self.writers.drain().await;
            guard.check_deadline()?;
            export(&guard).await
        })
        .await
        .map_err(|_| DbError::Internal("logical export maintenance deadline exceeded".into()))?
    }
}
