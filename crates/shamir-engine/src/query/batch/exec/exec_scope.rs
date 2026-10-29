//! Shared per-execution budget for the recursive executor chain.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use shamir_storage::error::DbError;

use crate::query::batch::result_size::ResultSize;
use crate::query::batch::{BatchError, BatchLimits, ExecutionDeadline};
use crate::query::filter::read_budget::{ReadByteMeter, ReadVisibilityScope};
use crate::query::read::{QueryRecord, QueryResult};

/// Per-read credit, never a delta of the shared root counter.
/// Intermediate charges are retained; only surviving, charged output rows
/// are credited against the result envelope at completion.
pub(crate) struct RowByteMeter<'a> {
    scope: &'a ExecScope,
    credited: AtomicU64,
}

impl<'a> RowByteMeter<'a> {
    pub(crate) fn new(scope: &'a ExecScope) -> Option<Self> {
        scope.enforce_result_size().then(|| Self {
            scope,
            credited: AtomicU64::new(0),
        })
    }

    pub(crate) fn value_size(value: &shamir_types::types::value::QueryValue) -> u64 {
        48u64.saturating_add(ResultSize::value(value))
    }

    pub(crate) fn record_size(record: &QueryRecord) -> u64 {
        match record {
            QueryRecord::Direct(value) => Self::value_size(value),
            QueryRecord::IdBytes(bytes) => 72u64.saturating_add(bytes.len() as u64),
            // Sinks never produce Inserted; keep its estimator authoritative.
            QueryRecord::Inserted(_) => ResultSize::result(&QueryResult {
                records: vec![record.clone()],
                ..QueryResult::default()
            })
            .saturating_sub(80),
        }
    }

    pub(crate) fn charge(&self, bytes: u64) -> Result<(), DbError> {
        // Io owns a typed Error source: no string conversion through DbResult.
        self.scope
            .charge_bytes(bytes)
            .map_err(|error| DbError::Io(std::io::Error::other(error)))
    }

    pub(crate) fn charge_record(&self, record: &QueryRecord) -> Result<(), DbError> {
        let bytes = Self::record_size(record);
        self.charge(bytes)?;
        self.credited.fetch_add(bytes, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn credit_survivors(&self, records: &[QueryRecord]) {
        let bytes = records.iter().fold(0u64, |total, record| {
            total.saturating_add(Self::record_size(record))
        });
        self.credited.store(bytes, Ordering::Relaxed);
    }

    /// Total = all materialized rows + result estimate - surviving row credit.
    /// Raw aggregate inputs have no output credit; the aggregate result pays
    /// in full. Shortcut reads with no sink charges also pay in full here.
    pub(crate) fn finish(&self, result: &QueryResult) -> Result<(), BatchError> {
        self.scope.charge_bytes(
            ResultSize::result(result).saturating_sub(self.credited.load(Ordering::Relaxed)),
        )
    }
}

/// One root budget shared by all nested batches and loop bodies.
/// Work and policy-enabled materialization bytes share the root budget.
#[derive(Debug)]
pub struct ExecScope {
    limits: BatchLimits,
    read_visibility_timeout_ms: u64,
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
            read_visibility_timeout_ms: 5000,
            deadline: ExecutionDeadline::from_budget_secs(limits.max_execution_time_secs),
            deadline_cap: None,
            work: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            enforce_result_size,
        }
    }

    /// Tighten the cooperative deadline without changing whole-second wire limits.
    pub(in crate::query::batch) fn with_deadline_cap(
        mut self,
        deadline: Instant,
        cap: Duration,
    ) -> Self {
        let budget = cap.min(Duration::from_secs(
            self.limits.max_execution_time_secs.max(1),
        ));
        // Preserve the attachment instant's deadline through synchronous setup.
        self.deadline_cap = Some((deadline, budget));
        self
    }

    /// Inspect the actual caller-cap deadline consulted by `check`.
    #[cfg(test)]
    pub(in crate::query::batch) fn cap_deadline(&self) -> Option<Instant> {
        self.deadline_cap.map(|(deadline, _)| deadline)
    }

    /// Inspect budget metadata; request/policy time is enforced independently.
    #[cfg(test)]
    pub(in crate::query::batch) fn capped_budget(&self) -> Option<Duration> {
        self.deadline_cap.map(|(_, budget)| budget)
    }

    /// Test-only duration injection with the deadline's exact zero/overflow semantics.
    #[cfg(test)]
    pub(crate) fn from_duration(limits: &BatchLimits, duration: Duration) -> Self {
        Self {
            limits: limits.clone(),
            read_visibility_timeout_ms: 5000,
            deadline: ExecutionDeadline::from_duration(duration),
            deadline_cap: None,
            work: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            enforce_result_size: false,
        }
    }

    pub(in crate::query::batch) fn with_read_visibility_timeout(mut self, timeout_ms: u64) -> Self {
        self.read_visibility_timeout_ms = timeout_ms;
        self
    }

    pub(crate) fn read_visibility_budget(&self) -> Duration {
        self.read_visibility_budget_at(Instant::now())
    }

    fn read_visibility_budget_at(&self, now: Instant) -> Duration {
        let mut budget = Duration::from_millis(self.read_visibility_timeout_ms);
        if let Some(remaining) = self.deadline.remaining_at(now) {
            budget = budget.min(remaining);
        }
        if let Some((deadline, _)) = self.deadline_cap {
            budget = budget.min(deadline.saturating_duration_since(now));
        }
        budget
    }

    #[cfg(test)]
    pub(in crate::query::batch) fn read_visibility_budget_at_for_test(
        &self,
        now: Instant,
    ) -> Duration {
        self.read_visibility_budget_at(now)
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

impl ReadVisibilityScope for ExecScope {
    fn read_visibility_budget(&self) -> Duration {
        Self::read_visibility_budget(self)
    }
}

impl ReadByteMeter for RowByteMeter<'_> {
    fn charge(&self, bytes: u64) -> Result<(), DbError> {
        Self::charge(self, bytes)
    }

    fn charge_record(&self, record: &QueryRecord) -> Result<(), DbError> {
        Self::charge_record(self, record)
    }

    fn credit_survivors(&self, records: &[QueryRecord]) {
        Self::credit_survivors(self, records);
    }
}
