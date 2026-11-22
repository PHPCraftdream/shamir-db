//! Typed busy gate around the server-side SCRAM Argon2id derivation
//! (brief ARGON2-GATE item 1 + the crate-internal core of item 2).
//!
//! [`ScramDerivationGate`] wraps the boot's existing `Argon2Semaphore`
//! (one pool, created in `launch.rs`) and serialises the only production
//! Argon2id callers (`CreateScramUser`, `UserAdminPort::create_user`).
//! The permit is acquired INSIDE the blocking closure and held around
//! the KDF only (spec §8.1), with a fixed bounded wait: on timeout —
//! or an unreadable clock, which the semaphore already reports the same
//! way — the derivation is refused with a typed
//! [`ScramDerivationError::Busy`], never queued forever and never a
//! panic. The RAII permit guarantees release on success, failure and
//! panic. The public gated entry point is
//! `derive_scram_record_gated`, which delegates to this gate; this
//! module keeps only the gate itself.

use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Fixed bounded wait for a permit: 3 s absorbs a short queue of
/// in-flight derivations while guaranteeing every caller gets a typed
/// refusal instead of queueing indefinitely (brief: fixed deadline, no
/// new configuration key).
const BUSY_DEADLINE_NS: u64 = 3 * ns::SECOND;

/// Typed refusal surface for a gated derivation. `Busy` carries no
/// payload on purpose: its Display text is the stable, safe client
/// message ("password derivation is busy; retry later") and must leak
/// neither the semaphore state nor the user name.
#[derive(Debug, thiserror::Error)]
pub enum ScramDerivationError {
    /// No permit could be acquired within the fixed busy deadline, or
    /// the deadline could not be computed (unreadable clock /
    /// overflow). Both fail closed as busy.
    #[error("password derivation is busy; retry later")]
    Busy,
    /// The derivation itself failed (KDF error or blocking-task join
    /// failure); carries the same internal message text the ungated
    /// `derive_scram_record` returned before.
    #[error("{0}")]
    Failed(String),
}

/// Gate in front of the server-side SCRAM key derivation. Owns a handle
/// to the boot's single `Argon2Semaphore` and counts busy refusals (the
/// future `shamir_argon2_busy_refusals_total` series source; scraped via
/// [`ScramDerivationGate::busy_refusals`]).
pub struct ScramDerivationGate {
    semaphore: Arc<Argon2Semaphore>,
    /// Total refusals handed out by the blocking `run` core.
    /// `Relaxed` is enough: the counter is advisory scrape data, and each
    /// refusal is already ordered internally before being published.
    busy_refusals: AtomicU64,
}

impl ScramDerivationGate {
    /// Wrap the boot's existing semaphore (one pool, not a second one).
    pub fn new(semaphore: Arc<Argon2Semaphore>) -> Self {
        Self {
            semaphore,
            busy_refusals: AtomicU64::new(0),
        }
    }

    /// Semaphore handle for the metrics scrape (`capacity` / `available`
    /// series are read from it at scrape time).
    pub fn semaphore(&self) -> &Argon2Semaphore {
        &self.semaphore
    }

    /// Refusals counted so far (relaxed load; advisory counter).
    pub fn busy_refusals(&self) -> u64 {
        self.busy_refusals.load(Ordering::Relaxed)
    }

    /// Blocking core: acquire a permit inside the caller's blocking
    /// context, hold it (RAII — released on success, error and unwind)
    /// around the KDF closure only, and map every failed acquisition —
    /// an unreadable clock, a deadline overflow, or an exhausted wait —
    /// to exactly one counted [`ScramDerivationError::Busy`]. Generic
    /// over the KDF outcome `T` so the closure seam can return the bare
    /// derived keys (record assembly stays outside the permit) and tests
    /// can inject a derivation error or a panic and prove permit release
    /// on every path.
    pub(super) fn run<T>(&self, kdf: impl FnOnce() -> T) -> Result<T, ScramDerivationError> {
        // Single Option chain: compute the deadline INSIDE the blocking
        // context (an unreadable clock or an overflow yields `None`) and
        // hand it straight to the semaphore. Any `None` — clock,
        // overflow, or permit timeout — is one busy refusal, counted
        // exactly once.
        let permit = UnixNanos::try_now()
            .ok()
            .and_then(|now| now.as_u64().checked_add(BUSY_DEADLINE_NS))
            .and_then(|deadline_ns| self.semaphore.acquire_until(deadline_ns));

        let _permit = match permit {
            Some(permit) => permit,
            None => {
                self.busy_refusals.fetch_add(1, Ordering::Relaxed);
                return Err(ScramDerivationError::Busy);
            }
        };

        Ok(kdf())
    }
}
