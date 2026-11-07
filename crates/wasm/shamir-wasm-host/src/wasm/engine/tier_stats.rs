//! [`TierStats`], [`TierReport`] and [`UpgradeState`] - upgrade counters and the per-function tier report.

use super::{Tier, TierMode};
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

/// Upgrade state machine of a function: at most one upgrade, one retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeState {
    /// No upgrade attempted yet.
    Idle = 0,
    /// An upgrade job is in flight.
    Scheduled = 1,
    /// The first upgrade failed; one 16x-threshold retry remains.
    Failed = 2,
    /// An optimized slot was installed.
    Done = 3,
    /// The retry failed as well; no further attempts.
    Exhausted = 4,
}

impl UpgradeState {
    /// Decodes the state byte stored in the function's atomic.
    pub(in crate::wasm) fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Idle,
            1 => Self::Scheduled,
            2 => Self::Failed,
            3 => Self::Done,
            4 => Self::Exhausted,
            _ => unreachable!("invalid upgrade state byte {raw}"),
        }
    }
}

/// Per-function tier snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierReport {
    /// Tier of the live slot.
    pub tier: Tier,
    /// Tier mode fixed at construction.
    pub mode: TierMode,
    /// Calls counted by the upgrade scheduler. Only an `Auto` function on an
    /// engine with a fast tier counts; the counter stops once an upgrade is
    /// scheduled, so it saturates near the upgrade threshold (calls racing the
    /// scheduling step may overshoot by a few) and stays 0 for `Now` and
    /// `Never`.
    pub calls_observed: u64,
    /// Where the single background upgrade stands.
    pub upgrade_state: UpgradeState,
}
