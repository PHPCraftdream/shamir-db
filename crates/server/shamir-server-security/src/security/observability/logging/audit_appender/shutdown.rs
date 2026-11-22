//! Ordered audit-appender shutdown: the stop request, the flusher join and
//! the final drain + anchor write as three separately observable steps.
//!
//! The persistence gate is a synchronous `parking_lot` mutex and the final
//! drain fsyncs, so a stalled checkpoint holds the gate for as long as its
//! own I/O takes. Splitting the steps lets the server put the blocking part
//! on a blocking thread, bound the whole stage with one deadline and report
//! which step failed instead of discarding the result. The gate rules
//! themselves, the anchor write and the latched fail-closed behaviour are
//! unchanged.

use super::{Durab, FjallAuditAppender};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Results of the appender's shutdown stage, in step order.
///
/// Deliberately not `#[must_use]`: boot-rollback and test callers that do
/// not own a shutdown report keep discarding it exactly as before.
#[derive(Clone, Debug, Default)]
pub struct AuditShutdownOutcome {
    /// The background flusher task could not be joined (panicked or
    /// cancelled): its buffered entries were drained by the final flush,
    /// but the task itself did not end cleanly.
    pub flusher_join_error: Option<String>,
    /// The final drain, anchor write or server-meta hook failed. The
    /// appender is latched fail-closed either way.
    pub flush_error: Option<String>,
}

impl AuditShutdownOutcome {
    /// Both steps completed without an error.
    pub fn is_ok(&self) -> bool {
        self.flusher_join_error.is_none() && self.flush_error.is_none()
    }
}

impl FjallAuditAppender {
    /// Ask the background flusher to stop and wake it.
    ///
    /// Synchronous and gate-free: it only flips the stop flag and notifies,
    /// so it is safe while another thread holds the persistence gate.
    pub fn request_flusher_stop(&self) {
        if let Durab::Batched {
            shutdown, notify, ..
        } = &self.mode
        {
            shutdown.store(true, Ordering::SeqCst);
            notify.notify_one();
        }
    }

    /// Await the background flusher's join handle.
    ///
    /// Pure async join: it takes no gate and blocks no worker thread, so a
    /// flusher parked on the persistence gate is bounded by the caller's
    /// deadline rather than by the gate holder's I/O.
    pub async fn join_flusher(&self) -> Result<(), String> {
        let handle = match &self.mode {
            Durab::Batched { task, .. } => task.lock().take(),
            Durab::Strict => None,
        };
        let Some(handle) = handle else {
            return Ok(());
        };
        handle.await.map_err(|error| error.to_string())
    }

    /// Final batched drain, anchor persistence and server-meta hook.
    ///
    /// Synchronous by design (it acquires the persistence gate and fsyncs):
    /// the caller runs it on a blocking thread so the shutdown future's poll
    /// is never the thread that waits for the gate. Retention sweeps stay
    /// rotation-only. Anchor and hook failures latch fail-closed.
    pub fn final_flush_blocking(&self) -> Result<(), String> {
        let result = self
            .flush_buffer_locked()
            .and_then(|()| self.persist_anchor_and_meta());
        if let Err(error) = &result {
            *self.failed.lock() = Some(error.to_string());
        }
        result.map_err(|error| error.to_string())
    }
}

/// Test-only arming of the persistence-gate hook across the crate
/// boundary: the hook runs while the caller already owns the gate, so a
/// test can hold a REAL checkpoint inside it and observe a bounded
/// teardown.
#[cfg(any(test, feature = "test-hooks"))]
impl FjallAuditAppender {
    /// Arm the one-shot hook run immediately after the persistence gate is
    /// acquired.
    pub fn arm_gate_acquired_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self.gate_acquired_hook.lock() = Some(hook);
    }

    /// Make the next actual flush fail (fault injection).
    pub fn arm_fail_next_flush(&self) {
        self.fail_next_flush.store(true, Ordering::SeqCst);
    }
}
