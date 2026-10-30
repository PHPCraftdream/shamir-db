use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use scc::HashMap;
use shamir_collections::THasher;
use shamir_storage::types::RecordKey;

// ============================================================================
// Level-3 pessimistic locking — wound-wait, deadlock-free by construction.
//
// Locks live in a SEPARATE map (`MvccStore::locks`), NOT in the hot-path
// `RecordCell`. The map is populated ONLY for keys locked by a Pessimistic
// (Level-3) transaction; it stays empty when no Level-3 tx runs, so the
// snapshot/serializable read/write paths pay zero overhead.
//
// Wound-wait: a requester only ever *waits* on strictly-older holders and
// only ever *wounds* strictly-younger ones (the tx's monotonic id is its
// priority — smaller id = older = higher priority). The wait-for graph
// therefore respects the total id order and cannot cycle, so no deadlock
// detector is needed.
// ============================================================================

/// Lock mode for a Level-3 pessimistic lock.
///
/// `Shared` is compatible with other `Shared` holders (multiple readers);
/// `Exclusive` is compatible with nothing but the same tx (re-entrant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
}

/// A single lock holder: the holding tx's monotonic id, its shared
/// `wounded` flag, and a per-tx `Notify` the wounder triggers so the
/// holder — which may be parked waiting on a DIFFERENT key — wakes up
/// and observes the wound. This is load-bearing for deadlock-freedom:
/// a wound issued on key Y must wake a tx parked on key X, so the wake
/// cannot be keyed on the lock where the wound happened.
#[derive(Debug)]
pub(crate) struct Holder {
    pub(crate) tx_version: u64,
    pub(crate) wounded: Arc<AtomicBool>,
    pub(crate) wound_notify: Arc<tokio::sync::Notify>,
}
/// The mutable state of one key's lock: the set of current holders plus
/// the aggregate mode (`None` when unheld). Invariant: when `mode` is
/// `Some(Exclusive)`, `holders` has exactly one entry; when `Some(Shared)`,
/// every holder is a distinct tx (no duplicate ids).
#[derive(Debug, Default)]
pub(crate) struct KeyLockState {
    pub(crate) holders: Vec<Holder>,
    pub(crate) mode: Option<LockMode>,
}

impl KeyLockState {
    /// True if `tx_version` already holds this key in ANY mode. Used for
    /// re-entrant upgrades/re-locks: a same-tx re-acquire is always allowed
    /// and never self-deadlocks.
    pub(crate) fn held_by(&self, tx_version: u64) -> bool {
        self.holders.iter().any(|h| h.tx_version == tx_version)
    }

    /// Recompute `mode` from the surviving holders. `None` when empty;
    /// `Shared` when more than one holder (the invariant guarantees the
    /// only multi-holder mode is Shared); otherwise leave the existing
    /// mode (a lone holder is whatever the caller last requested).
    pub(crate) fn recompute_mode(&mut self) {
        match self.holders.len() {
            0 => self.mode = None,
            1 => {}
            _ => self.mode = Some(LockMode::Shared),
        }
    }
}

/// Per-key pessimistic lock. Guards [`KeyLockState`] under a `tokio::sync`
/// `Mutex` (the sanctioned exception — the guard lives across the
/// `.await` on `notify.notified()` and contention is bounded by the
/// wound-wait protocol). `Notify` wakes every waiter on each release/wound
/// so they re-evaluate compatibility.
#[derive(Debug)]
pub struct KeyLock {
    pub(crate) state: tokio::sync::Mutex<KeyLockState>,
    pub(crate) notify: tokio::sync::Notify,
    // Modified under the registry entry lock, except holders which mirrors state.
    leases: AtomicUsize,
    pub(crate) holders: AtomicUsize,
}

impl KeyLock {
    pub(crate) fn new() -> Self {
        Self {
            state: tokio::sync::Mutex::new(KeyLockState::default()),
            notify: tokio::sync::Notify::new(),
            leases: AtomicUsize::new(0),
            holders: AtomicUsize::new(0),
        }
    }
}

/// Pins the registry identity through every await, including cancellation.
pub(crate) struct LockLease<'a> {
    registry: &'a HashMap<RecordKey, Arc<KeyLock>, THasher>,
    key: RecordKey,
    pub(crate) lock: Arc<KeyLock>,
}

impl<'a> LockLease<'a> {
    pub(crate) fn acquire(
        registry: &'a HashMap<RecordKey, Arc<KeyLock>, THasher>,
        key: RecordKey,
        insert: bool,
    ) -> Option<Self> {
        // Sync entry operations cannot hand a bucket lock to a suspended task;
        // that handoff can deadlock runtime workers using synchronous access.
        let lock = match registry.entry_sync(key.clone()) {
            scc::hash_map::Entry::Occupied(entry) => {
                entry.get().leases.fetch_add(1, Ordering::Relaxed);
                Arc::clone(entry.get())
            }
            scc::hash_map::Entry::Vacant(entry) if insert => {
                let lock = Arc::new(KeyLock::new());
                lock.leases.store(1, Ordering::Relaxed);
                entry.insert_entry(Arc::clone(&lock));
                lock
            }
            scc::hash_map::Entry::Vacant(_) => return None,
        };
        Some(Self {
            registry,
            key,
            lock,
        })
    }
}

impl Drop for LockLease<'_> {
    fn drop(&mut self) {
        if let scc::hash_map::Entry::Occupied(entry) = self.registry.entry_sync(self.key.clone()) {
            let lock = entry.get();
            // Acquisition and retirement share the entry lock; all state users
            // hold leases, and publish the holder count before releasing them.
            debug_assert!(Arc::ptr_eq(lock, &self.lock));
            let previous = lock.leases.fetch_sub(1, Ordering::Relaxed);
            if previous == 1 && lock.holders.load(Ordering::Acquire) == 0 {
                let _ = entry.remove_entry();
            }
        }
    }
}
