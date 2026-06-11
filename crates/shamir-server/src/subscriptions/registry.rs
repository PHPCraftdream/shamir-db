use std::sync::atomic::{AtomicU64, Ordering};

use scc::HashMap as SccHashMap;
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
pub(crate) struct SubscriptionRegistry {
    subs: SccHashMap<u64, ActiveSubscription>,
    next_id: AtomicU64,
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self {
            subs: SccHashMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn insert(&self, id: u64, sub: ActiveSubscription) {
        let _ = self.subs.insert(id, sub);
    }

    pub fn remove(&self, id: u64) -> bool {
        self.subs.remove(&id).is_some()
    }

    /// Cancel all subscriptions (connection teardown).
    pub fn close_all(&self) {
        self.subs.retain(|_, _| false);
    }

    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.subs.len()
    }
}
