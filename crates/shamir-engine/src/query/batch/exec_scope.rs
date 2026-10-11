//! Shared per-execution budget for the recursive executor chain.

use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::time::Duration;

use super::{BatchError, BatchLimits, ExecutionDeadline};

/// One root budget shared by all nested batches and loop bodies.
/// Counters are scaffolding only; later stages own work/byte enforcement.
#[derive(Debug)]
pub struct ExecScope {
    limits: BatchLimits,
    deadline: ExecutionDeadline,
    work: AtomicU64,
    bytes: AtomicU64,
}

impl ExecScope {
    /// Preserve the supplied limits and start their cooperative time budget.
    pub fn root(limits: &BatchLimits) -> Self {
        Self {
            limits: limits.clone(),
            deadline: ExecutionDeadline::from_budget_secs(limits.max_execution_time_secs),
            work: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }

    /// Test-only duration injection with the deadline's exact zero/overflow semantics.
    #[cfg(test)]
    pub(crate) fn from_duration(limits: &BatchLimits, duration: Duration) -> Self {
        Self {
            limits: limits.clone(),
            deadline: ExecutionDeadline::from_duration(duration),
            work: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }

    /// Borrow the limits owned by this execution scope.
    pub fn limits(&self) -> &BatchLimits {
        &self.limits
    }

    /// Work consumed so far (no charging in this refactor).
    pub fn work(&self) -> u64 {
        // Independent telemetry counter, not a payload publication mechanism.
        self.work.load(Ordering::Relaxed)
    }

    /// Bytes consumed so far (no charging in this refactor).
    pub fn bytes(&self) -> u64 {
        // Independent telemetry counter, not a payload publication mechanism.
        self.bytes.load(Ordering::Relaxed)
    }

    /// Delegate the existing cooperative checkpoint without changing its errors.
    pub fn check(&self) -> Result<(), BatchError> {
        self.deadline.check()
    }
}
