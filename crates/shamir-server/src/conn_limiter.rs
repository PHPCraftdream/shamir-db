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
