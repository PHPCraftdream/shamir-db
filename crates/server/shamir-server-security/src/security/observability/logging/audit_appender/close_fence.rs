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
//!
//! That read alone is a check-then-write race: a checkpoint that passes it can
//! park before taking the persistence gate while `final_flush_blocking` takes
//! the gate and persists the final anchor, and the resumed checkpoint would
//! then write over it. `refuse_closed_checkpoint_under_gate` is the DECISIVE
//! half: `checkpoint` re-reads the same atomic INSIDE its
//! `persistence_section` closure, i.e. under the gate the close takes, so the
//! re-test is serialized with the final anchor write. It takes no other lock
//! (the close holds only that gate, so there is no new lock order), and each
//! refused call is counted once: the cheap half returned `Ok` for any call
//! that reaches the decisive half.
//!
//! `fire_before_gate_checkpoint_hook` is a `#[cfg(test)]` rendezvous point
//! between the two halves (its slot, `before_gate_checkpoint_hook`, does not
//! exist in production builds).

use super::{AppenderError, Durab, FjallAuditAppender};
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

    /// DECISIVE half of the fence: `checkpoint` calls this INSIDE its
    /// `persistence_section` closure, i.e. while it owns the persistence gate
    /// that `final_flush_blocking` also takes, and before `checkpoint_locked`.
    /// A checkpoint that passed `refuse_closed_checkpoint` and parked before
    /// the gate is therefore refused here instead of writing the anchor after
    /// the close. It reads one atomic and takes no other lock.
    ///
    /// The refusal is counted here at most once per call: the cheap half
    /// returns `Ok` for every call that reaches this point. It is surfaced as
    /// [`AppenderError::Io`] because the gate closure returns `AppenderError`;
    /// `latch_checkpoint_failure` recognizes it and does not latch it.
    pub(super) fn refuse_closed_checkpoint_under_gate(&self) -> Result<(), AppenderError> {
        if !self.closed_for_checkpoints() {
            return Ok(());
        }
        self.refused_checkpoint_attempts
            .fetch_add(1, Ordering::SeqCst);
        Err(AppenderError::Io(std::io::Error::other(ClosedCheckpoint)))
    }

    /// Latch a failed public `checkpoint` exactly as the trait method always
    /// did, except for the post-close refusal: that one is not a persistence
    /// failure (the close already persisted the final anchor), so it must not
    /// reach `last_failure()` and be reported again by the shutdown stage.
    pub(super) fn latch_checkpoint_failure(&self, error: std::io::Error) -> std::io::Error {
        if !is_closed_checkpoint(&error) {
            *self.failed.lock() = Some(error.to_string());
        }
        error
    }

    /// Run and disarm the one-shot end-of-append hook (test builds only).
    #[cfg(test)]
    pub(super) fn fire_after_append_hook(&self) {
        let hook = self.after_append_hook.lock().take();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Run and disarm the one-shot pre-gate checkpoint hook (test builds
    /// only): `checkpoint` fires it AFTER the cheap fence and BEFORE
    /// `acquire_persistence_gate`, so a test can close the appender in that
    /// window. The slot is `#[cfg(test)]`, so nothing fires in production.
    #[cfg(test)]
    pub(super) fn fire_before_gate_checkpoint_hook(&self) {
        let hook = self.before_gate_checkpoint_hook.lock().take();
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// `true` when `error` is the post-close checkpoint refusal, as returned by
/// [`FjallAuditAppender::refuse_closed_checkpoint`] or wrapped by the gate
/// closure's [`AppenderError::Io`] and the trait method's `io::Error::other`.
fn is_closed_checkpoint(error: &std::io::Error) -> bool {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<AppenderError>())
    {
        Some(AppenderError::Io(inner)) => inner
            .get_ref()
            .is_some_and(|source| source.downcast_ref::<ClosedCheckpoint>().is_some()),
        _ => false,
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

    /// Arm the one-shot hook run INSIDE `checkpoint` after the cheap pre-gate
    /// fence and BEFORE the persistence gate (`acquire_persistence_gate`), so
    /// a test can close the appender while a checkpoint is parked in that
    /// window. The hook slot itself is `#[cfg(test)]`, so this method cannot
    /// fire in a production build.
    #[cfg(test)]
    pub fn arm_before_gate_checkpoint_hook(&self, hook: std::sync::Arc<dyn Fn() + Send + Sync>) {
        *self.before_gate_checkpoint_hook.lock() = Some(hook);
    }
}
