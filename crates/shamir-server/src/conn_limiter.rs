//! Global cap on simultaneously-active connections.
//!
//! A simple atomic counter incremented at TCP-accept and decremented when
//! the per-connection task ends. RAII guard means we can't forget to
//! decrement on early exits (TLS handshake failure, slow-loris timeout,
//! handler panic — all clean up via Drop).
//!
//! Why **before** the TLS handshake: TLS itself costs ~5–10 ms of CPU per
//! connection. If the server is already at the cap, accepting a TCP
//! connection just to throw it away after the TLS work is wasteful and
//! plays into a DoS attacker's hand. We close the socket immediately
//! (TCP RST) and burn a single accept syscall instead.
//!
//! `try_acquire()` is non-blocking — it returns `None` rather than
//! waiting. The accept loop logs and drops the socket; the attacker's
//! retries see the same RST until the cap clears.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Cap-tracker. Cheaply cloneable — wraps an `Arc<AtomicUsize>` and a
/// fixed maximum.
#[derive(Debug, Clone)]
pub struct ConnLimiter {
    active: Arc<AtomicUsize>,
    cap: usize,
}

impl ConnLimiter {
    /// New limiter with the given hard cap. `cap == 0` is treated as
    /// "no limit" (every `try_acquire` returns `Some`).
    pub fn new(cap: usize) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            cap,
        }
    }

    /// Try to reserve a slot for one new connection. Returns `Some(guard)`
    /// on success — drop the guard to release the slot. Returns `None`
    /// when the cap is reached.
    ///
    /// Implementation note: we use a CAS-free `fetch_add` + rollback
    /// pattern. Two callers racing into the cap-1 slot can both increment
    /// (transiently exceed by 1), then one rolls back. Worst case: the
    /// counter briefly shows `cap + N` for a few μs. Acceptable: the cap
    /// is a coarse DoS shield, not a precise resource accountant.
    pub fn try_acquire(&self) -> Option<ConnGuard> {
        if self.cap == 0 {
            // No cap configured — return a no-op guard so callers can
            // pretend the limiter is always "on".
            return Some(ConnGuard {
                limiter: self.clone(),
                active: false,
            });
        }
        let prev = self.active.fetch_add(1, Ordering::Relaxed);
        if prev >= self.cap {
            self.active.fetch_sub(1, Ordering::Relaxed);
            None
        } else {
            Some(ConnGuard {
                limiter: self.clone(),
                active: true,
            })
        }
    }

    /// Current count of active connections. Cheap atomic load.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Configured hard cap (or 0 if unlimited).
    pub fn cap(&self) -> usize {
        self.cap
    }
}

/// RAII guard for one occupied connection slot. `Drop` decrements the
/// counter — guaranteed to fire even on panic.
pub struct ConnGuard {
    limiter: ConnLimiter,
    active: bool,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        if self.active {
            self.limiter.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_zero_is_unlimited() {
        let l = ConnLimiter::new(0);
        let _g1 = l.try_acquire().expect("first acquire");
        let _g2 = l.try_acquire().expect("second acquire");
        let _g3 = l.try_acquire().expect("third acquire");
        // Active stays 0 because guards are no-ops.
        assert_eq!(l.active(), 0);
    }

    #[test]
    fn cap_3_allows_3_then_rejects() {
        let l = ConnLimiter::new(3);
        let g1 = l.try_acquire().expect("1");
        let g2 = l.try_acquire().expect("2");
        let g3 = l.try_acquire().expect("3");
        assert_eq!(l.active(), 3);
        assert!(l.try_acquire().is_none(), "4th must be rejected");

        // Drop one — next acquire succeeds.
        drop(g1);
        assert_eq!(l.active(), 2);
        let _g4 = l.try_acquire().expect("after release");
        assert_eq!(l.active(), 3);
        let _ = (g2, g3);
    }

    #[test]
    fn drop_releases_slot() {
        let l = ConnLimiter::new(1);
        {
            let _g = l.try_acquire().expect("first");
            assert_eq!(l.active(), 1);
            assert!(l.try_acquire().is_none(), "cap reached");
        }
        // Out of scope — slot freed.
        assert_eq!(l.active(), 0);
        let _g = l.try_acquire().expect("after drop");
    }

    #[test]
    fn limiter_clones_share_state() {
        let l1 = ConnLimiter::new(2);
        let l2 = l1.clone();
        let _g1 = l1.try_acquire().expect("from l1");
        assert_eq!(l2.active(), 1, "l2 sees l1's acquire");
        let _g2 = l2.try_acquire().expect("from l2");
        assert!(l1.try_acquire().is_none(), "l1 sees l2's acquire");
    }
}
