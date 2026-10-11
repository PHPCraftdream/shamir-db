//! Shared per-execution budget for the recursive executor chain.

use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::time::Duration;

use super::{BatchError, BatchLimits, ExecutionDeadline};

/// One root budget shared by all nested batches and loop bodies.
/// Work is enforced at dispatch; byte accounting remains scaffolding.
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

    /// Work consumed so far, including the dispatch that exceeded the cap.
    pub fn work(&self) -> u64 {
        // Independent telemetry counter, not a payload publication mechanism.
        self.work.load(Ordering::Relaxed)
    }

    /// Charge dispatched work against the shared root budget.
    pub fn charge_work(&self, n: u64) -> Result<(), BatchError> {
        // This atomic only accounts for a budget, never publishes payload data.
        // Relaxed suffices; each dispatch pays exactly one atomic add.
        let previous = self.work.fetch_add(n, Ordering::Relaxed);
        match previous.checked_add(n) {
            Some(actual) if actual <= self.limits.max_total_work => Ok(()),
            total => {
                if total.is_none() {
                    // Cold overflow path: do not leave a wrapped, small budget.
                    self.work.fetch_max(u64::MAX, Ordering::Relaxed);
                }
                Err(BatchError::LimitExceeded {
                    kind: shamir_query_types::batch::LimitKind::Work,
                    actual: total.unwrap_or(u64::MAX),
                    max: self.limits.max_total_work,
                })
            }
        }
    }

    /// Check prospective work without reserving or charging it.
    pub fn check_work(&self, extra: u64) -> Result<(), BatchError> {
        // Not a reservation: nested dispatches still charge their actual work.
        match self.work.load(Ordering::Relaxed).checked_add(extra) {
            Some(actual) if actual <= self.limits.max_total_work => Ok(()),
            total => Err(BatchError::LimitExceeded {
                kind: shamir_query_types::batch::LimitKind::Work,
                actual: total.unwrap_or(u64::MAX),
                max: self.limits.max_total_work,
            }),
        }
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
