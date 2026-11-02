use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Shared cap for request-body bytes currently admitted across connections.
#[derive(Debug)]
pub struct BodyBudget {
    /// Maximum number of bytes admitted concurrently across requests.
    cap: usize,
    /// Current aggregate reservation, in bytes.
    in_use: AtomicUsize,
}

impl BodyBudget {
    /// Creates a body budget with the given byte cap.
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            in_use: AtomicUsize::new(0),
        }
    }

    /// Reserves exactly `n` bytes, or returns `None` without changing accounting.
    pub fn try_reserve(self: &Arc<Self>, n: usize) -> Option<BodyReservation> {
        let mut current = self.in_use.load(Ordering::Acquire);
        loop {
            let next = current.checked_add(n)?;
            if next > self.cap {
                return None;
            }
            match self.in_use.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(BodyReservation {
                        budget: self.clone(),
                        bytes: n,
                    })
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// Number of bytes currently reserved.
    pub fn in_use(&self) -> usize {
        self.in_use.load(Ordering::Acquire)
    }
}

/// RAII reservation returned to its budget on drop.
#[derive(Debug)]
pub struct BodyReservation {
    /// Shared budget retained for this reservation.
    budget: Arc<BodyBudget>,
    /// Number of bytes currently reserved.
    bytes: usize,
}

impl BodyReservation {
    /// Currently reserved byte count for this request.
    pub fn reserved_bytes(&self) -> usize {
        self.bytes
    }
    /// Extends the reservation atomically.
    pub fn grow(&mut self, n: usize) -> bool {
        let Some(next_bytes) = self.bytes.checked_add(n) else {
            return false;
        };
        let mut current = self.budget.in_use.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(n) else {
                return false;
            };
            if next > self.budget.cap {
                return false;
            }
            match self.budget.in_use.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.bytes = next_bytes;
                    return true;
                }
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for BodyReservation {
    fn drop(&mut self) {
        self.budget.in_use.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
