//! [`TierStats`] — atomic upgrade counters, readable by tests.

use std::sync::atomic::AtomicU64;

/// Counters for the background tier-upgrade lifecycle.
#[derive(Debug, Default)]
pub struct TierStats {
    /// Upgrade jobs that entered the bounded scheduler.
    pub upgrades_started: AtomicU64,
    /// Upgrade jobs that swapped an optimized slot in.
    pub upgrades_completed: AtomicU64,
    /// Upgrade jobs that failed and kept the fast slot.
    pub upgrades_failed: AtomicU64,
}
