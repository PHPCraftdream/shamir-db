//! Shared admission for a server's logical-export freeze.
use super::guard::MaintenanceGuard;
use futures::future::BoxFuture;
use shamir_engine_table::writer_drain_barrier::{WriterDrainBarrier, WriterDrainGuard};
use shamir_storage::error::{DbError, DbResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
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
#[derive(Clone, Default)]
pub struct MaintenanceGate {
    frozen: Arc<AtomicBool>,
    writers: WriterDrainBarrier,
}

impl MaintenanceGate {
    /// Refuse new mutations during maintenance. Increment BEFORE checking the
    /// intent flag: the existing SeqCst drain protocol then catches every writer
    /// that observed the old flag, without duplicating its counter/barrier.
    pub fn enter_mutation(&self) -> DbResult<WriterDrainGuard> {
        let writer = self.writers.enter_writer();
        if self.frozen.load(Ordering::SeqCst) {
            return Err(DbError::Conflict(
                "logical export maintenance in progress".into(),
            ));
        }
        Ok(writer)
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
        let guard = MaintenanceGuard::new(Arc::clone(&self.frozen), deadline);
        tokio::time::timeout_at(deadline, async {
            self.writers.drain().await;
            guard.check_deadline()?;
            export(&guard).await
        })
        .await
        .map_err(|_| DbError::Internal("logical export maintenance deadline exceeded".into()))?
    }
}
