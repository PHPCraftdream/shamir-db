//! Synchronous publication/termination interlock.
//!
//! The mutex spans the actual publication callback: termination waits for
//! an in-flight callback to finish, then becomes sticky. Callbacks must be
//! synchronous and must not block waiting for async work or re-enter lifecycle operations.

use std::sync::{Mutex, MutexGuard};

/// Returned when termination has closed publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationClosed;

/// Serializes publication callbacks with sticky termination.
#[derive(Default)]
pub struct PubTermInterlock {
    gate: Mutex<bool>,
}

impl PubTermInterlock {
    /// Creates an open interlock.
    pub const fn new() -> Self {
        Self {
            gate: Mutex::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        self.gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Runs the synchronous publication callback while holding the gate.
    /// Callback panic releases the guard via RAII; termination cannot return until the callback completes.
    pub fn publish<T>(&self, callback: impl FnOnce() -> T) -> Result<T, PublicationClosed> {
        let terminated = self.lock();
        if *terminated {
            return Err(PublicationClosed);
        }
        Ok(callback())
    }

    /// Sticky termination; waits for any callback already holding the gate.
    pub fn terminate(&self) -> bool {
        let mut terminated = self.lock();
        let newly_terminated = !*terminated;
        *terminated = true;
        newly_terminated
    }

    /// Returns whether publication has been terminated.
    pub fn is_terminated(&self) -> bool {
        *self.lock()
    }

    /// Deliberately unsafe-ordering negative-control seam for deterministic tests;
    /// separates admission-check from callback execution.
    #[cfg(test)]
    pub(crate) fn unprotected_publish_for_test<T>(
        &self,
        after_check: impl FnOnce(),
        callback: impl FnOnce() -> T,
    ) -> Result<T, PublicationClosed> {
        if self.is_terminated() {
            return Err(PublicationClosed);
        }
        after_check();
        Ok(callback())
    }
}
