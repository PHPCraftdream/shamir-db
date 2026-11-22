//! Typed admission and busy gate around server-side SCRAM Argon2id derivation.
//!
//! [`ScramDerivationGate`] wraps the boot's existing `Argon2Semaphore`
//! (one pool, created in `launch.rs`) and gates the only production Argon2id
//! callers (`CreateScramUser`, `UserAdminPort::create_user`). Admission is
//! bounded before `spawn_blocking`: at most twice the semaphore capacity may
//! be running or waiting for a permit. An admitted job owns an RAII ticket and
//! one permit budget that is anchored at admission on BOTH clocks — the
//! wall-clock deadline and the monotonic instant of the same reading — and
//! both are retained through the whole blocking job even if the request future
//! is cancelled. Queue time therefore counts, and a backward wall-clock step
//! cannot hand the job a fresh budget.

use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::server::argon2_semaphore::{Argon2PermitBudget, Argon2Semaphore};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Fixed bounded permit budget measured from admission (brief: fixed deadline,
/// no new configuration key).
const BUSY_DEADLINE_NS: u64 = 3 * ns::SECOND;

/// Typed refusal surface for a gated derivation. `Busy` carries no payload on
/// purpose: its Display text is the stable, safe client message and must leak
/// neither the semaphore state nor the user name.
#[derive(Debug, thiserror::Error)]
pub enum ScramDerivationError {
    /// Admission was full, the permit budget elapsed, or its clock could not
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
    /// Permit budget fixed by this admission on both clocks; the blocking job
    /// passes it to the semaphore unchanged.
    pub(super) budget: Argon2PermitBudget,
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

    /// Reserve an admission slot and fix its permit budget before the caller
    /// submits any blocking work. The clock seam keeps deadline tests
    /// deterministic; production always supplies the system Unix clock.
    pub(super) fn admit(self: &Arc<Self>) -> Result<AdmissionTicket, ScramDerivationError> {
        self.admit_with_clock(|| UnixNanos::try_now().ok().map(|now| now.as_u64()))
    }

    /// `now` returns the current wall clock in Unix nanoseconds. The budget's
    /// deadline is derived from that one reading, so the monotonic anchor, the
    /// wall-clock reading and the wall deadline all describe the same instant.
    pub(super) fn admit_with_clock(
        self: &Arc<Self>,
        now: impl FnOnce() -> Option<u64>,
    ) -> Result<AdmissionTicket, ScramDerivationError> {
        // Clock first: an unreadable clock or an overflowing deadline rejects
        // the request before it reserves anything, so no slot can leak.
        let Some(admitted_ns) = now() else {
            return Err(self.busy());
        };
        let Some(deadline_ns) = admitted_ns
            .checked_add(BUSY_DEADLINE_NS)
            .filter(|deadline_ns| *deadline_ns != u64::MAX)
        else {
            return Err(self.busy());
        };

        // Bounded, not isolated: the admission cap is twice the semaphore
        // capacity (up to 2,048 with the maximum configured capacity of
        // 1,024), while both server runtimes build the Tokio runtime with the
        // default blocking-pool limit of 512 threads. Above a capacity of 256
        // this backlog alone can fill that shared pool and queue unrelated
        // blocking work (WAL sync, HTTP verifier lookups) behind it; nothing
        // here reserves or isolates pool threads.
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

        Ok(AdmissionTicket {
            gate: Arc::clone(self),
            budget: Argon2PermitBudget::at_admission(deadline_ns, admitted_ns),
        })
    }

    /// Blocking core: acquire the permit against the budget fixed at
    /// admission (queue time already charged on both clocks), hold the permit
    /// around the KDF, and map every failed acquisition to exactly one counted
    /// `Busy`.
    pub(super) fn run<T>(
        &self,
        budget: Argon2PermitBudget,
        kdf: impl FnOnce() -> T,
    ) -> Result<T, ScramDerivationError> {
        let Some(_permit) = self.semaphore.acquire_until_admitted(budget) else {
            return Err(self.busy());
        };
        Ok(kdf())
    }
}
