use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use scc::HashMap as SccHashMap;
use shamir_collections::THasher;
use shamir_tunables::instance_defaults::MAX_SUBSCRIPTIONS_PER_CONNECTION;
use tokio::task::JoinHandle;

/// An active subscription with its bridge task handle.
///
/// `bridge_handle` starts `None` — a slot is [`SubscriptionRegistry::reserve_pending`]'d
/// (map entry present, no handle yet) BEFORE the bridge task is spawned, and
/// [`SubscriptionRegistry::attach_handle`] fills it in right after
/// `tokio::spawn` returns. See the struct doc on `SubscriptionRegistry` for
/// why this split exists (closes a spawn-vs-insert race found in `@fl`
/// review of task #527's self-exit slot-release fix).
pub(crate) struct ActiveSubscription {
    pub bridge_handle: Option<JoinHandle<()>>,
}

impl Drop for ActiveSubscription {
    fn drop(&mut self) {
        if let Some(handle) = self.bridge_handle.take() {
            handle.abort();
        }
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
    closed: AtomicBool,
    /// Maximum concurrently-active subscriptions this connection may hold.
    cap: usize,
}

impl Default for SubscriptionRegistry {
    fn default() -> Self {
        Self {
            subs: SccHashMap::with_hasher(THasher::default()),
            next_id: AtomicU64::new(1),
            active: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            cap: MAX_SUBSCRIPTIONS_PER_CONNECTION,
        }
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
            closed: AtomicBool::new(false),
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
            if self.closed.load(Ordering::Acquire) || current >= cap {
                return Err(cap);
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if self.closed.load(Ordering::Acquire) {
                        self.release_reservation();
                        return Err(cap);
                    }
                    return Ok(());
                }
                Err(observed) => current = observed,
            }
        }
    }

    /// Reserve a handle-less placeholder slot for `id` before spawning.
    /// Returns `false` and releases the already-reserved slot if the registry
    /// is closed or the id is duplicated.
    pub(crate) fn reserve_pending(&self, id: u64) -> bool {
        if self.closed.load(Ordering::Acquire) {
            self.release_reservation();
            return false;
        }
        if self
            .subs
            .insert_sync(
                id,
                ActiveSubscription {
                    bridge_handle: None,
                },
            )
            .is_err()
        {
            self.release_reservation();
            return false;
        }
        if self.closed.load(Ordering::Acquire) {
            self.remove(id);
            return false;
        }
        true
    }

    fn release_reservation(&self) {
        let previous = self.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }

    /// Attach the real bridge-task handle to an already-`reserve_pending`'d
    /// slot. If the slot is missing or the registry is closed, abort the task
    /// and remove any remaining slot.
    pub(crate) fn attach_handle(&self, id: u64, handle: JoinHandle<()>) {
        let mut handle = Some(handle);
        self.subs.update_sync(&id, |_, sub| {
            if !self.closed.load(Ordering::Acquire) {
                sub.bridge_handle = handle.take();
            }
        });
        if let Some(handle) = handle {
            handle.abort();
            self.remove(id);
        }
    }

    pub fn remove(&self, id: u64) -> bool {
        if self.subs.remove_sync(&id).is_some() {
            self.release_reservation();
            true
        } else {
            false
        }
    }

    /// Cancel all subscriptions (connection teardown).
    pub fn close_all(&self) {
        self.closed.store(true, Ordering::Release);
        self.subs.retain_sync(|_, _| {
            self.release_reservation();
            false
        });
    }

    /// Number of currently-active subscriptions (O(1)).
    pub fn count(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}
