//! Post-close checkpoint fence for the public `checkpoint` entry point.
//!
//! A batched `append_entry` admitted before the close is answered from the
//! buffer, and the writer's inline checkpoint (`every 1000 entries`) runs
//! after it. That checkpoint can therefore resume AFTER
//! `final_flush_blocking` persisted the anchor and persist the anchor a
//! second time. `append_entry` already refuses once the batched `shutdown`
//! flag is set; the public `checkpoint` did not, so the fence here closes
//! that window.
//!
//! The fence is a single atomic read in the same `SeqCst` order as
//! `append_entry`'s guard and `request_flusher_stop`'s store. It takes no
//! gate and touches no store, so the close's own writes are unaffected:
//! `final_flush_blocking` drains through `flush_buffer_locked` and anchors
//! through `persist_anchor_and_meta` -> `write_checkpoint`, never through the
//! public trait method. `Durab::Strict` has no shutdown flag, so its
//! `checkpoint` is byte-for-byte unchanged.

use super::{Durab, FjallAuditAppender};
use std::sync::atomic::Ordering;

/// Text of the typed post-close checkpoint refusal. The writer maps any
/// appender error to `AuditError::Persistence`; this text is only for
/// diagnosis.
const CLOSED_CHECKPOINT_REFUSAL: &str =
    "audit appender is shut down; refusing a post-close checkpoint";

/// Typed refusal: the batched appender is closed, so its anchor is final.
#[derive(Debug)]
pub(super) struct ClosedCheckpoint;

impl core::fmt::Display for ClosedCheckpoint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(CLOSED_CHECKPOINT_REFUSAL)
    }
}

impl std::error::Error for ClosedCheckpoint {}

impl FjallAuditAppender {
    /// `true` once a batched close was requested (`request_flusher_stop`
    /// stored the flag). Strict mode has no shutdown flag: always `false`.
    fn closed_for_checkpoints(&self) -> bool {
        match &self.mode {
            Durab::Strict => false,
            Durab::Batched { shutdown, .. } => shutdown.load(Ordering::SeqCst),
        }
    }

    /// Fence for the public `checkpoint` path. Refuses WITHOUT taking the
    /// persistence gate or touching the store, and counts the refusal so a
    /// resume after the close is observable.
    pub(super) fn refuse_closed_checkpoint(&self) -> Result<(), std::io::Error> {
        if !self.closed_for_checkpoints() {
            return Ok(());
        }
        self.refused_checkpoint_attempts
            .fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other(ClosedCheckpoint))
    }

    /// Run and disarm the one-shot end-of-append hook (test builds only).
    #[cfg(test)]
    pub(super) fn fire_after_append_hook(&self) {
        let hook = self.after_append_hook.lock().take();
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// Test-visible observables for the post-close fence.
#[cfg(any(test, feature = "test-hooks"))]
impl FjallAuditAppender {
    /// How many public `checkpoint` calls the post-close fence refused.
    /// Stays 0 while the appender is open (the fence is a pure read then).
    pub fn refused_checkpoint_attempts(&self) -> usize {
        self.refused_checkpoint_attempts.load(Ordering::SeqCst)
    }

    /// Arm the one-shot hook run at the END of a successful batched
    /// `append_entry` (after its buffer push and after the buffer lock is
    /// released, so a parked hook holds no lock). The hook slot itself is
    /// `#[cfg(test)]`, so this method cannot fire in a production build.
    #[cfg(test)]
    pub fn arm_after_append_hook(&self, hook: std::sync::Arc<dyn Fn() + Send + Sync>) {
        *self.after_append_hook.lock() = Some(hook);
    }
}
