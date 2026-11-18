//! Admission decision types for [`crate::server::state::session::SessionStore::admit`]
//! (spec §7.4 overflow policy: idle-LRU eviction or all-active rejection).
//!
//! One admission function serves login AND resume (RV-CONNECT-8AD8B1C8).
//! `AdmitReject` + `ACTIVE_WINDOW_NS` live in the COMMON layer
//! (`crate::common::model::admit`) so `Error::AdmissionRejected` can carry
//! the reason in client-only builds; they are re-exported here so every
//! existing `server::admission::{...}` path stays stable.

use smallvec::SmallVec;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::common::types::limits;
use crate::server::state::session::Session;

pub use crate::common::admit::{AdmitReject, ACTIVE_WINDOW_NS};

/// Session-id key type (bearer token).
pub type Sid = [u8; limits::SESSION_ID_BYTES];

/// RAII per-session in-flight slot (R4b-1 follow-up): acquired via
/// [`Session::try_acquire_request_slot`], released on drop. Held across
/// `.await` (the whole batch execution), so it owns no lock — release is one
/// `fetch_sub` on the session's shared `Arc<AtomicUsize>` counter, which a
/// same-bearer replacement generation inherits (see `Session`'s
/// `in_flight_requests` field doc), so a panic or early return on any
/// generation still releases against the same cell.
pub struct SessionRequestSlot {
    counter: Arc<AtomicUsize>,
}

impl Drop for SessionRequestSlot {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Session {
    /// Atomically admit one more in-flight request for this session, up to
    /// `cap` concurrently. `Some(slot)` = admitted (the slot is held until
    /// the returned guard drops); `None` = the cap is exhausted, caller
    /// rejects the request before any engine work. Lock-free CAS loop.
    ///
    /// `cap == 0` admits NOTHING — zero does not disable the cap. The
    /// counter is shared with a same-bearer replacement generation (`admit` swaps it in under the
    /// occupied guard before publication), so a re-login cannot raise the
    /// effective concurrency while old requests still run; a fresh session
    /// (token origin, stateless REST with no stored handle) starts empty.
    pub fn try_acquire_request_slot(&self, cap: usize) -> Option<SessionRequestSlot> {
        let counter = &self.in_flight_requests;
        if cap == 0 {
            return None;
        }
        loop {
            let current = counter.load(Ordering::Relaxed);
            if current >= cap {
                return None;
            }
            match counter.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                // Own the shared cell only once admission succeeds — no
                // refcount traffic on the rejected/contended path.
                Ok(_) => {
                    return Some(SessionRequestSlot {
                        counter: Arc::clone(counter),
                    })
                }
                // Lost a race with a concurrent acquire/release: re-read.
                Err(_) => continue,
            }
        }
    }
}

/// Outcome of [`crate::server::state::session::SessionStore::admit`].
#[derive(Debug)]
pub enum Admission {
    /// Session admitted. `evicted` lists the idle LRU victims removed to
    /// make room; `replaced` is true when the same sid was re-admitted for
    /// the same user (a replacement consumes no extra capacity).
    Admitted {
        /// The admitted session.
        session: Arc<Session>,
        /// Sids evicted (idle LRU, spec §7.4).
        evicted: SmallVec<[Sid; 1]>,
        /// Same sid + same user re-admission.
        replaced: bool,
    },
    /// Rejected — mutates nothing apart from idempotent index pruning.
    Rejected(AdmitReject),
}
