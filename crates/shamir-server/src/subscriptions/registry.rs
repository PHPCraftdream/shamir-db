use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use scc::HashMap as SccHashMap;
use shamir_collections::THasher;
use shamir_tunables::instance_defaults::MAX_SUBSCRIPTIONS_PER_CONNECTION;
use tokio::task::JoinHandle;

/// An active subscription with its bridge task handle.
pub(crate) struct ActiveSubscription {
    pub bridge_handle: JoinHandle<()>,
}

impl Drop for ActiveSubscription {
    fn drop(&mut self) {
        self.bridge_handle.abort();
    }
}

/// Per-connection subscription registry.
/// Uses `scc::HashMap` for lock-free concurrent access.
pub struct SubscriptionRegistry {
    subs: SccHashMap<u64, ActiveSubscription, THasher>,
    next_id: AtomicU64,
    /// O(1) live-subscription counter (mirror of `subs` cardinality).
    /// `scc::HashMap::len()` is O(N), so we keep this atomic in sync at
    /// every insert / remove / close_all to enforce the per-connection cap
    /// (finding 2b-i) without a full traversal on the hot subscribe path.
    active: AtomicUsize,
    /// Maximum concurrently-active subscriptions this connection may hold.
    cap: usize,
}

impl Default for SubscriptionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self::with_cap(MAX_SUBSCRIPTIONS_PER_CONNECTION)
    }

    /// Construct with an explicit per-connection subscription cap (tests).
    pub fn with_cap(cap: usize) -> Self {
        Self {
            subs: SccHashMap::with_hasher(THasher::default()),
            next_id: AtomicU64::new(1),
            active: AtomicUsize::new(0),
            cap,
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The per-connection subscription cap.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Reserve one subscription slot, failing if the connection is already
    /// at its cap (finding 2b-i). On success the caller MUST follow with
    /// [`Self::insert`]; on `Err` no slot is consumed. A CAS loop keeps the
    /// check-and-reserve atomic so concurrent `Subscribe` ops on one
    /// connection cannot both slip past the cap.
    pub(crate) fn try_reserve(&self) -> Result<(), usize> {
        let cap = self.cap;
        let mut current = self.active.load(Ordering::Relaxed);
        loop {
            if current >= cap {
                return Err(cap);
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    pub(crate) fn insert(&self, id: u64, sub: ActiveSubscription) {
        if self.subs.insert(id, sub).is_err() {
            // Duplicate id (never expected: ids are monotonic) — release the
            // slot reserved for this insert so the counter stays accurate.
            self.active.fetch_sub(1, Ordering::AcqRel);
        }
    }

    pub fn remove(&self, id: u64) -> bool {
        if self.subs.remove(&id).is_some() {
            self.active.fetch_sub(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

    /// Cancel all subscriptions (connection teardown).
    pub fn close_all(&self) {
        self.subs.retain(|_, _| false);
        self.active.store(0, Ordering::Release);
    }

    /// Number of currently-active subscriptions (O(1)).
    pub fn count(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}
