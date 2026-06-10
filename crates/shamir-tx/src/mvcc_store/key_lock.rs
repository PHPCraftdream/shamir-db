use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
}

impl KeyLock {
    pub(crate) fn new() -> Self {
        Self {
            state: tokio::sync::Mutex::new(KeyLockState::default()),
            notify: tokio::sync::Notify::new(),
        }
    }
}
