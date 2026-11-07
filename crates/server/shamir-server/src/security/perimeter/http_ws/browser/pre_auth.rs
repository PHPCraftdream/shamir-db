//! Bounded RAII admission for browser first-frame authentication.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use shamir_collections::THasher;
use shamir_connect::server::lockout::{subnet_of, Subnet};

const GLOBAL_LIMIT: usize = 256;
const SUBNET_LIMIT: usize = 8;

/// Global and per-subnet pre-authentication capacity counters.
pub(crate) struct PreAuthAdmission {
    global: AtomicUsize,
    subnet_counts: DashMap<Subnet, Arc<AtomicUsize>, THasher>,
}

impl Default for PreAuthAdmission {
    fn default() -> Self {
        Self {
            global: AtomicUsize::new(0),
            subnet_counts: DashMap::with_hasher(THasher::default()),
        }
    }
}

impl PreAuthAdmission {
    /// Reserve one bounded browser first-frame slot or reject without allocating token data.
    pub(crate) fn try_acquire(
        self: &Arc<Self>,
        address: std::net::IpAddr,
    ) -> Option<PreAuthPermit> {
        reserve(&self.global, GLOBAL_LIMIT)?;
        let subnet = subnet_of(address);
        let counter = {
            let entry = self
                .subnet_counts
                .entry(subnet)
                .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
            if reserve(entry.value(), SUBNET_LIMIT).is_none() {
                drop(entry);
                self.global.fetch_sub(1, Ordering::AcqRel);
                return None;
            }
            Arc::clone(entry.value())
        };
        Some(PreAuthPermit {
            admission: Arc::clone(self),
            subnet,
            counter,
        })
    }
    /// Returns current global and subnet reservations for deterministic tests.
    #[cfg(test)]
    pub(crate) fn in_use(&self, address: std::net::IpAddr) -> (usize, usize) {
        let subnet = subnet_of(address);
        let subnet_count = self
            .subnet_counts
            .get(&subnet)
            .map_or(0, |counter| counter.load(Ordering::Acquire));
        (self.global.load(Ordering::Acquire), subnet_count)
    }
}

fn reserve(counter: &AtomicUsize, limit: usize) -> Option<()> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(1).filter(|next| *next <= limit)
        })
        .ok()
        .map(|_| ())
}

/// Holds one global and one subnet pre-authentication slot until dropped.
pub(crate) struct PreAuthPermit {
    admission: Arc<PreAuthAdmission>,
    subnet: Subnet,
    counter: Arc<AtomicUsize>,
}

impl Drop for PreAuthPermit {
    fn drop(&mut self) {
        let previous = self.counter.fetch_sub(1, Ordering::AcqRel);
        self.admission.global.fetch_sub(1, Ordering::AcqRel);
        if previous == 1 {
            self.admission
                .subnet_counts
                .remove_if(&self.subnet, |_, current| {
                    Arc::ptr_eq(current, &self.counter) && current.load(Ordering::Acquire) == 0
                });
        }
    }
}
