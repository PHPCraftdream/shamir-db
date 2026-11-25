use shamir_storage::error::{DbError, DbResult};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::Instant;

/// Unforgeable, borrowed capability valid only inside `with_freeze`.
/// Dropping the enclosing future releases it even during drain or a page read.
/// Every exit (success, error, deadline, cancellation, panic/unwind of the
/// enclosing task) drops this guard, which clears the intent flag AND wakes
/// every queued `wait_for_mutation` waiter via `notify_waiters`.
pub struct MaintenanceGuard {
    frozen: Arc<AtomicBool>,
    thaw: Arc<Notify>,
    deadline: Instant,
    metadata_bytes: AtomicUsize,
}

impl MaintenanceGuard {
    pub(super) fn new(frozen: Arc<AtomicBool>, thaw: Arc<Notify>, deadline: Instant) -> Self {
        Self {
            frozen,
            thaw,
            deadline,
            metadata_bytes: AtomicUsize::new(0),
        }
    }

    pub(super) fn charge_metadata(&self, bytes: usize) -> DbResult<()> {
        self.check_deadline()?;
        self.metadata_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= 64 * 1024 * 1024)
            })
            .map_err(|_| DbError::Validation("logical export metadata exceeds limit".into()))?;
        Ok(())
    }

    pub(super) fn check_deadline(&self) -> DbResult<()> {
        if Instant::now() >= self.deadline {
            return Err(DbError::Internal(
                "logical export maintenance deadline exceeded".into(),
            ));
        }
        Ok(())
    }
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        // Clear the flag FIRST so a woken waiter re-checks a consistent
        // (thawed) state, then wake ALL queued waiters. `notify_waiters`
        // (not `notify_one`) is required: several tasks may be queued, and
        // `notify_waiters` stores no permit, so a waiter that has not yet
        // armed its `Notified` future simply re-checks the flag on its own
        // loop and does not depend on this wakeup.
        self.frozen.store(false, Ordering::SeqCst);
        self.thaw.notify_waiters();
    }
}
