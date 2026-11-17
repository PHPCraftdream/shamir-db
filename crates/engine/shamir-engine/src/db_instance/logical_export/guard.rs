use shamir_storage::error::{DbError, DbResult};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::time::Instant;

/// Unforgeable, borrowed capability valid only inside `with_freeze`.
/// Dropping the enclosing future releases it even during drain or a page read.
pub struct MaintenanceGuard {
    frozen: Arc<AtomicBool>,
    deadline: Instant,
    metadata_bytes: AtomicUsize,
}

impl MaintenanceGuard {
    pub(super) fn new(frozen: Arc<AtomicBool>, deadline: Instant) -> Self {
        Self {
            frozen,
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
        self.frozen.store(false, Ordering::SeqCst);
    }
}
