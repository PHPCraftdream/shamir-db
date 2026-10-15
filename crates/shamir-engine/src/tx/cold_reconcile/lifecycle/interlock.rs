//! Publication/termination interlock (S5c-X PR6b-2b).
//!
//! A control-plane object shared between the lifecycle owner and the worker
//! pipeline's publication path. Zero hot-path cost: atomics only, no locks,
//! no allocation after construction.
//!
//! Semantics: at most one publication section may be in flight at a time,
//! and quarantine (termination) is unconditional and sticky — once
//! termination is requested, no NEW publication section may begin. A
//! publication already in flight when termination is requested is allowed
//! to complete; no further publication can start afterwards.
//!
//! Linearization: `terminate()` first sets the sticky `requested` flag
//! (Release), then CASes the state. Either
//! * the publication completes first (`end_publication` flips
//!   PUBLISHING→IDLE, observes `requested`, and CASes IDLE→TERMINATED), so
//!   `terminate()`'s CAS loses and returns false — but no further
//!   publication can begin; or
//! * quarantine wins (`terminate()` CASes IDLE→TERMINATED and returns
//!   true) — the in-flight publication has already ended.
//! There is no interleaving in which a publication begins after a
//! termination was requested. Conversely, a benign conflict (another
//! publication in flight) NEVER terminates the interlock: `requested` is
//! only made sticky when the observed state is `TERMINATED`.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Publication/termination interlock — see the module doc for the contract.
#[derive(Default)]
pub struct PubTermInterlock {
    /// `IDLE` | `PUBLISHING` | `TERMINATED`.
    state: AtomicU8,
    /// Sticky "termination requested" flag set before any state CAS.
    requested: AtomicBool,
}

impl PubTermInterlock {
    /// State: no publication in flight.
    pub const IDLE: u8 = 0;
    /// State: exactly one publication section in flight.
    pub const PUBLISHING: u8 = 1;
    /// State: terminated — quarantine won.
    pub const TERMINATED: u8 = 2;

    /// Creates an idle, non-terminated interlock.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(Self::IDLE),
            requested: AtomicBool::new(false),
        }
    }

    /// Enters a publication section. Returns `false` if termination was
    /// requested (quarantine wins over NEW publications) or if the
    /// interlock is already terminated, or if another publication section
    /// is in flight.
    pub fn begin_publication(&self) -> bool {
        if self.requested.load(Ordering::Acquire) {
            return false;
        }
        if self
            .state
            .compare_exchange(
                Self::IDLE,
                Self::PUBLISHING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            true
        } else {
            // CAS lost: re-read the state to distinguish a benign
            // PUBLISHING conflict from a TERMINATED interlock. Only a
            // genuinely terminated state is made sticky (uniformity after
            // quarantine wins); a concurrent publication in flight must
            // NOT terminate the interlock — the contender's
            // `end_publication` returns the interlock to IDLE and
            // publication stays open for later sections.
            if self.state.load(Ordering::Acquire) == Self::TERMINATED {
                self.requested.store(true, Ordering::Release);
            }
            false
        }
    }

    /// Leaves a publication section. If termination was requested while
    /// publishing, this completes the quarantine by CASing IDLE→TERMINATED.
    pub fn end_publication(&self) {
        if self
            .state
            .compare_exchange(
                Self::PUBLISHING,
                Self::IDLE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
            && self.requested.load(Ordering::Acquire)
        {
            let _ = self.state.compare_exchange(
                Self::IDLE,
                Self::TERMINATED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    /// Requests unconditional termination. Returns `true` when THIS call
    /// won the terminal transition (or it was already terminated); `false`
    /// when a publication was in flight — the publication completes first
    /// and finishes the termination on its way out (see module doc).
    pub fn terminate(&self) -> bool {
        self.requested.store(true, Ordering::Release);
        match self.state.compare_exchange(
            Self::IDLE,
            Self::TERMINATED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => true,
            Err(prev) => prev == Self::TERMINATED,
        }
    }

    /// `true` once termination has been requested or won.
    pub fn is_terminated(&self) -> bool {
        self.requested.load(Ordering::Acquire)
            || self.state.load(Ordering::Acquire) == Self::TERMINATED
    }
}
