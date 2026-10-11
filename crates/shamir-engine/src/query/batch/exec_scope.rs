//! Shared per-execution budget for the recursive executor chain.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{BatchError, BatchLimits, ExecutionDeadline};

/// One root budget shared by all nested batches and loop bodies.
/// Work and policy-enabled materialization bytes share the root budget.
#[derive(Debug)]
pub struct ExecScope {
    limits: BatchLimits,
    deadline: ExecutionDeadline,
    deadline_cap: Option<(Instant, Duration)>,
    work: AtomicU64,
    bytes: AtomicU64,
    enforce_result_size: bool,
}

impl ExecScope {
    /// Preserve the supplied limits and start their cooperative time budget.
    pub fn root(limits: &BatchLimits) -> Self {
        Self::root_with_result_size(limits, false)
    }

    /// Enable byte enforcement only for a caller-owned execution policy.
    pub fn root_with_result_size(limits: &BatchLimits, enforce_result_size: bool) -> Self {
        Self {
            limits: limits.clone(),
            deadline: ExecutionDeadline::from_budget_secs(limits.max_execution_time_secs),
            deadline_cap: None,
            work: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            enforce_result_size,
        }
    }

    /// Tighten the cooperative deadline without changing whole-second wire limits.
    pub(super) fn with_deadline_cap(mut self, deadline: Instant, cap: Duration) -> Self {
        let budget = cap.min(Duration::from_secs(
            self.limits.max_execution_time_secs.max(1),
        ));
        // Preserve the attachment instant's deadline through synchronous setup.
        self.deadline_cap = Some((deadline, budget));
        self
    }

    /// Inspect the actual caller-cap deadline consulted by `check`.
    #[cfg(test)]
    pub(super) fn cap_deadline(&self) -> Option<Instant> {
        self.deadline_cap.map(|(deadline, _)| deadline)
    }

    /// Inspect budget metadata; request/policy time is enforced independently.
    #[cfg(test)]
    pub(super) fn capped_budget(&self) -> Option<Duration> {
        self.deadline_cap.map(|(_, budget)| budget)
    }

    /// Test-only duration injection with the deadline's exact zero/overflow semantics.
    #[cfg(test)]
    pub(crate) fn from_duration(limits: &BatchLimits, duration: Duration) -> Self {
        Self {
            limits: limits.clone(),
            deadline: ExecutionDeadline::from_duration(duration),
            deadline_cap: None,
            work: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            enforce_result_size: false,
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

    /// Whether insertion should run the estimator (OFF never traverses values).
    pub fn enforce_result_size(&self) -> bool {
        self.enforce_result_size
    }

    /// Charge bytes materialized, including intermediates; never give bytes back.
    pub fn charge_bytes(&self, n: u64) -> Result<(), BatchError> {
        if !self.enforce_result_size {
            return Ok(());
        }
        // Budget only, not payload publication. Mirror W6's cold overflow path.
        let previous = self.bytes.fetch_add(n, Ordering::Relaxed);
        match previous.checked_add(n) {
            Some(actual) if actual <= self.limits.max_result_size as u64 => Ok(()),
            total => {
                if total.is_none() {
                    self.bytes.fetch_max(u64::MAX, Ordering::Relaxed);
                }
                Err(BatchError::LimitExceeded {
                    kind: shamir_query_types::batch::LimitKind::ResultBytes,
                    actual: total.unwrap_or(u64::MAX),
                    max: self.limits.max_result_size as u64,
                })
            }
        }
    }

    /// Bytes consumed so far, including the charge that exceeded the cap.
    pub fn bytes(&self) -> u64 {
        // Independent telemetry counter, not a payload publication mechanism.
        self.bytes.load(Ordering::Relaxed)
    }

    /// Check the legacy deadline and any exact caller-owned cap cooperatively.
    pub fn check(&self) -> Result<(), BatchError> {
        self.deadline.check()?;
        if let Some((deadline, budget)) = self.deadline_cap {
            if Instant::now() >= deadline {
                return Err(BatchError::ExecutionTimedOut {
                    budget_secs: budget.as_secs(),
                });
            }
        }
        Ok(())
    }
}
