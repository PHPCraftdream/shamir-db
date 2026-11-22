//! Typed admission and busy gate around server-side SCRAM Argon2id derivation.
//!
//! [`ScramDerivationGate`] wraps the boot's existing `Argon2Semaphore`
//! (one pool, created in `launch.rs`) and gates the only production Argon2id
//! callers (`CreateScramUser`, `UserAdminPort::create_user`). Admission is
//! bounded before `spawn_blocking`: at most twice the semaphore capacity may
//! be running or waiting for a permit. An admitted job owns an RAII ticket and
//! one absolute three-second permit deadline, both retained through the whole
//! blocking job even if the request future is cancelled.

use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Fixed bounded permit wait measured from admission (brief: fixed deadline,
/// no new configuration key).
const BUSY_DEADLINE_NS: u64 = 3 * ns::SECOND;

/// Typed refusal surface for a gated derivation. `Busy` carries no payload on
/// purpose: its Display text is the stable, safe client message and must leak
/// neither the semaphore state nor the user name.
#[derive(Debug, thiserror::Error)]
pub enum ScramDerivationError {
    /// Admission was full, the permit deadline elapsed, or its clock could not
    /// be read/computed; all fail closed as busy.
    #[error("password derivation is busy; retry later")]
    Busy,
    /// The derivation itself failed (KDF error or blocking-task join failure).
    #[error("{0}")]
    Failed(String),
}

/// Gate in front of server-side SCRAM key derivation. Tracks refused requests
/// from both bounded admission and permit acquisition.
pub struct ScramDerivationGate {
    semaphore: Arc<Argon2Semaphore>,
    /// Total refused requests (admission refusals and permit acquisition
    /// failures); this advisory scrape counter needs only relaxed ordering.
    busy_refusals: AtomicU64,
    /// Jobs admitted, including those running and queued for a semaphore
    /// permit. Numeric-only capacity enforcement needs no synchronization with
    /// other state.
    admitted: AtomicU32,
}

/// RAII admission owned by a blocking job. Dropping it releases one bounded
/// admission slot, including on unwind or after a cancelled caller.
pub(super) struct AdmissionTicket {
    gate: Arc<ScramDerivationGate>,
    pub(super) deadline_ns: u64,
}

impl Drop for AdmissionTicket {
    fn drop(&mut self) {
        self.gate.admitted.fetch_sub(1, Ordering::Relaxed);
    }
}

impl ScramDerivationGate {
    /// Wrap the boot's existing semaphore (one pool, not a second one).
    pub fn new(semaphore: Arc<Argon2Semaphore>) -> Self {
        Self {
            semaphore,
            busy_refusals: AtomicU64::new(0),
            admitted: AtomicU32::new(0),
        }
    }

    /// Semaphore handle for the metrics scrape (`capacity` / `available`
    /// series are read from it at scrape time).
    pub fn semaphore(&self) -> &Argon2Semaphore {
        &self.semaphore
    }

    /// Refused requests so far (relaxed load; advisory counter).
    pub fn busy_refusals(&self) -> u64 {
        self.busy_refusals.load(Ordering::Relaxed)
    }

    fn busy(&self) -> ScramDerivationError {
        self.busy_refusals.fetch_add(1, Ordering::Relaxed);
        ScramDerivationError::Busy
    }

    /// Reserve an admission slot and compute its absolute deadline before the
    /// caller submits any blocking work. The clock seam keeps deadline tests
    /// deterministic; production always supplies the system Unix clock.
    pub(super) fn admit(self: &Arc<Self>) -> Result<AdmissionTicket, ScramDerivationError> {
        self.admit_with_clock(|| {
            UnixNanos::try_now()
                .ok()
                .and_then(|now| now.as_u64().checked_add(BUSY_DEADLINE_NS))
                .filter(|deadline_ns| *deadline_ns != u64::MAX)
        })
    }

    pub(super) fn admit_with_clock(
        self: &Arc<Self>,
        deadline: impl FnOnce() -> Option<u64>,
    ) -> Result<AdmissionTicket, ScramDerivationError> {
        let max_admitted = self.semaphore.capacity().saturating_mul(2);
        let mut current = self.admitted.load(Ordering::Relaxed);
        loop {
            if current >= max_admitted {
                return Err(self.busy());
            }
            match self.admitted.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }

        // Establish the guard before clocking so every failure releases the
        // reservation exactly once.
        let mut ticket = AdmissionTicket {
            gate: Arc::clone(self),
            deadline_ns: 0,
        };
        let Some(deadline_ns) = deadline() else {
            let error = self.busy();
            drop(ticket);
            return Err(error);
        };
        ticket.deadline_ns = deadline_ns;
        Ok(ticket)
    }

    /// Blocking core: acquire the permit using the deadline fixed at
    /// admission, hold the permit around the KDF, and map every failed
    /// acquisition to exactly one counted `Busy`.
    pub(super) fn run<T>(
        &self,
        deadline_ns: u64,
        kdf: impl FnOnce() -> T,
    ) -> Result<T, ScramDerivationError> {
        let Some(_permit) = self.semaphore.acquire_until(deadline_ns) else {
            return Err(self.busy());
        };
        Ok(kdf())
    }
}
