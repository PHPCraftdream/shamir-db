//! Publication/termination interlock (S5c-X PR6b-2b, single-CAS design).
//!
//! A control-plane object shared between the lifecycle owner and the worker
//! pipeline's publication path. Zero hot-path cost: one atomic byte, no
//! locks, no allocation after construction.
//!
//! # State machine
//!
//! `IDLE -> PUBLISHING -> IDLE` for each publication section, and
//! `IDLE | PUBLISHING -> TERMINATED` for quarantine. NOTHING ever CASes out
//! of `TERMINATED`: termination is unconditional and sticky.
//!
//! # INVARIANT (the whole point of the single-CAS design)
//!
//! * `begin_publication` succeeds only from `IDLE`, via ONE
//!   `compare_exchange(IDLE, PUBLISHING)` — that CAS is the linearization
//!   point; there is no precheck load and no second flag.
//! * `terminate()` returns `true` only in a state where
//!   `state == TERMINATED` (either it won the CAS itself, or it observed
//!   `TERMINATED` on the retry loop).
//! * Therefore, once `terminate()` returns, `state == TERMINATED`, and no
//!   publication can EVER begin again.
//!
//! The previous two-flag design (`requested` AtomicBool plus a
//! load-then-CAS split in `begin_publication`) allowed a stale contender
//! that loaded the flag before termination was requested to interleave its
//! CAS into an IDLE window after `terminate()`'s store — a publication
//! beginning after termination was requested. That interleaving is
//! structurally impossible here: there is no second flag and no IDLE window
//! during pending termination — the state travels IDLE→TERMINATED in a
//! single CAS, or the caller spins until it observes TERMINATED.
//!
//! An in-flight publication when `terminate()` is called is cut short:
//! `terminate` CASes PUBLISHING→TERMINATED and returns `false` (the
//! section's already-issued work stands, but the section is terminal and
//! `end_publication` finds no `PUBLISHING` state to CAS).

use std::sync::atomic::{AtomicU8, Ordering};

/// Publication/termination interlock — see the module doc for the contract.
#[derive(Default)]
pub struct PubTermInterlock {
    /// `IDLE` | `PUBLISHING` | `TERMINATED`.
    state: AtomicU8,
}

impl PubTermInterlock {
    /// State: no publication in flight.
    pub const IDLE: u8 = 0;
    /// State: exactly one publication section in flight.
    pub const PUBLISHING: u8 = 1;
    /// State: terminated — quarantine won. Nothing CASes out of this state.
    pub const TERMINATED: u8 = 2;

    /// Creates an idle, non-terminated interlock.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(Self::IDLE),
        }
    }

    /// Enters a publication section via ONE CAS (`IDLE -> PUBLISHING`,
    /// AcqRel/Acquire) — that CAS is the linearization point. Returns
    /// `false` when the interlock is already `PUBLISHING` (another section
    /// in flight) or `TERMINATED` (quarantine won); no separate precheck,
    /// no sticky flag, no stale-contender window.
    pub fn begin_publication(&self) -> bool {
        self.state
            .compare_exchange(
                Self::IDLE,
                Self::PUBLISHING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Leaves a publication section. Best-effort CAS (`PUBLISHING -> IDLE`);
    /// the result is ignored because the state may already be `TERMINATED`
    /// — quarantine may have cut the section short.
    pub fn end_publication(&self) {
        let _ = self.state.compare_exchange(
            Self::PUBLISHING,
            Self::IDLE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Unconditional, sticky termination. Returns `true` only in a state
    /// where `state == TERMINATED`:
    /// 1. CAS `IDLE -> TERMINATED` won → `true` (quarantine won).
    /// 2. Load observed `TERMINATED` → `true` (already terminated).
    /// 3. Load observed `PUBLISHING` → CAS `PUBLISHING -> TERMINATED`:
    ///    ok → `false` (an in-flight section's work may already be issued,
    ///    but the section is terminal: `end_publication` finds no
    ///    `PUBLISHING` state and no NEW publication can ever begin because
    ///    nothing CASes out of `TERMINATED`); failed (a racing
    ///    end/terminate moved the state) → loop again.
    pub fn terminate(&self) -> bool {
        loop {
            if self
                .state
                .compare_exchange(
                    Self::IDLE,
                    Self::TERMINATED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
            match self.state.load(Ordering::Acquire) {
                Self::TERMINATED => return true,
                Self::PUBLISHING => {
                    if self
                        .state
                        .compare_exchange(
                            Self::PUBLISHING,
                            Self::TERMINATED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return false;
                    }
                    // A racing end_publication/terminate moved the state
                    // between our load and our CAS: loop and retry.
                }
                _ => continue,
            }
        }
    }

    /// `true` once the interlock is terminated.
    pub fn is_terminated(&self) -> bool {
        self.state.load(Ordering::Acquire) == Self::TERMINATED
    }

    /// Test-only negative control: replays EXACTLY the begin CAS
    /// (`IDLE -> PUBLISHING`). Used to prove deterministically that after
    /// `terminate()` no publication can begin.
    #[cfg(test)]
    pub(crate) fn stale_begin_cas_for_test(&self) -> bool {
        self.state
            .compare_exchange(
                Self::IDLE,
                Self::PUBLISHING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}
