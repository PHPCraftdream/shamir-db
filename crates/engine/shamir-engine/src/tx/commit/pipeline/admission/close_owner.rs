//! `CommitAdmissionCloseOwner` — RAII close right (design section 2,
//! "Close"; fail-closed recovery (iv)): created before any suspension
//! point of the seal, reopens on drop — ordinary error, timeout,
//! cancellation or unwind. The ACTIVE count is preserved across
//! close/reopen.

use std::sync::Arc;

use super::gate::CommitAdmissionGate;

/// Held only after `close()` observed zero pre-close-admitted leases.
/// Drop reopens the gate (clears CLOSED, then CLOSE_TAKEN) and wakes the
/// parked leaders. Uniqueness is guaranteed by the CLOSE_TAKEN bit: a
/// second `close()` got `CommitAdmissionError::Busy` and never received an
/// owner.
pub(crate) struct CommitAdmissionCloseOwner {
    gate: Arc<CommitAdmissionGate>,
}

impl CommitAdmissionCloseOwner {
    pub(crate) fn new(gate: Arc<CommitAdmissionGate>) -> Self {
        Self { gate }
    }
}

impl Drop for CommitAdmissionCloseOwner {
    fn drop(&mut self) {
        self.gate.reopen();
    }
}
