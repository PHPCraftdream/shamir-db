//! Lock-free visibility-acknowledgement telemetry.
//!
//! All recording is relaxed atomic increments/loads. The Strict/Visible fast
//! path pays exactly ONE relaxed increment (the result counter); duration
//! histograms are observed ONLY when an acknowledgement actually waited.
//! Time and histogram observations never happen on the fast path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::mvcc_store::write_receipt::{VisibilityPendingReason, VisibilityState};

/// Acknowledgement wait policy label space. Never version/tx/repo names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckMode {
    Strict,
    Bounded,
}

impl AckMode {
    pub fn label(self) -> &'static str {
        match self {
            AckMode::Strict => "strict",
            AckMode::Bounded => "bounded",
        }
    }
}

/// Terminal observation of one acknowledgement or reconciliation wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckResult {
    Visible,
    PendingTimeout,
    PendingReopen,
    Error,
}

impl AckResult {
    pub fn label(self) -> &'static str {
        match self {
            AckResult::Visible => "visible",
            AckResult::PendingTimeout => "pending_timeout",
            AckResult::PendingReopen => "pending_reopen",
            AckResult::Error => "error",
        }
    }

    /// A settled sample never records a wait; an observation error recorded
    /// no duration either.
    pub fn observed_wait(self) -> bool {
        !matches!(self, AckResult::Error)
    }
}

/// Why a Pending outcome was answered. Own label space: the tx layer and the
/// query DTOs each have their own pending-reason type, and each converts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingReason {
    Timeout,
    ReopenRequired,
}

impl PendingReason {
    pub fn label(self) -> &'static str {
        match self {
            PendingReason::Timeout => "timeout",
            PendingReason::ReopenRequired => "reopen_required",
        }
    }
}

impl From<VisibilityPendingReason> for PendingReason {
    fn from(reason: VisibilityPendingReason) -> Self {
        match reason {
            VisibilityPendingReason::Timeout => PendingReason::Timeout,
            VisibilityPendingReason::ReopenRequired => PendingReason::ReopenRequired,
        }
    }
}

/// Which write family produced a Pending outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    ExplicitTx,
    ImplicitBatch,
    DirectWrite,
}

impl PendingKind {
    pub fn label(self) -> &'static str {
        match self {
            PendingKind::ExplicitTx => "explicit_tx",
            PendingKind::ImplicitBatch => "implicit_batch",
            PendingKind::DirectWrite => "direct_write",
        }
    }
}

pub const ACK_MODES: usize = 2;
pub const ACK_RESULTS: usize = 4;
pub const PENDING_REASONS: usize = 2;
pub const PENDING_KINDS: usize = 3;

/// Sub-millisecond up to the configured 30 s maximum wait, plus overflow.
pub const BUCKET_BOUNDS_MS: [u64; 14] = [
    1, 2, 5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 15_000, 30_000,
];
pub const BUCKET_COUNT: usize = BUCKET_BOUNDS_MS.len() + 1;

/// Fixed-bucket duration histogram of atomics. Cumulative counts.
#[derive(Debug, Default)]
pub struct DurationHistogram {
    buckets: [AtomicU64; BUCKET_COUNT],
}

impl DurationHistogram {
    pub fn observe_ms(&self, ms: u64) {
        let index = bucket_index(ms);
        self.buckets[index].fetch_add(1, Ordering::Relaxed);
    }

    pub fn total(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    pub fn snapshot(&self) -> [u64; BUCKET_COUNT] {
        let mut out = [0_u64; BUCKET_COUNT];
        for (out, bucket) in out.iter_mut().zip(&self.buckets) {
            *out = bucket.load(Ordering::Relaxed);
        }
        out
    }
}

/// Cumulative bucket index: buckets are `<= bound` (Prometheus `le`).
pub fn bucket_index(ms: u64) -> usize {
    BUCKET_BOUNDS_MS
        .iter()
        .position(|&bound| ms <= bound)
        .unwrap_or(BUCKET_COUNT - 1)
}

#[derive(Debug, Default)]
struct AckCounters {
    total: [[AtomicU64; ACK_RESULTS]; ACK_MODES],
    wait_ms: [[DurationHistogram; ACK_RESULTS]; ACK_MODES],
}

impl AckCounters {
    fn record(&self, mode: AckMode, result: AckResult, waited: bool, elapsed: Duration) {
        let (mode_i, result_i) = (mode_index(mode), result_index(result));
        self.total[mode_i][result_i].fetch_add(1, Ordering::Relaxed);
        if waited && result.observed_wait() {
            self.wait_ms[mode_i][result_i].observe_ms(elapsed.as_millis() as u64);
        }
    }

    #[allow(clippy::type_complexity)] // fixed-shape label arrays
    fn snapshot(
        &self,
    ) -> (
        [[u64; ACK_RESULTS]; ACK_MODES],
        [[[u64; BUCKET_COUNT]; ACK_RESULTS]; ACK_MODES],
    ) {
        let mut total = [[0_u64; ACK_RESULTS]; ACK_MODES];
        let mut wait_ms = [[[0_u64; BUCKET_COUNT]; ACK_RESULTS]; ACK_MODES];
        for (mode_i, mode) in total.iter_mut().enumerate() {
            for (result_i, cell) in mode.iter_mut().enumerate() {
                *cell = self.total[mode_i][result_i].load(Ordering::Relaxed);
                wait_ms[mode_i][result_i] = self.wait_ms[mode_i][result_i].snapshot();
            }
        }
        (total, wait_ms)
    }
}

#[derive(Debug, Default)]
struct PendingCounters {
    total: [[AtomicU64; PENDING_KINDS]; PENDING_REASONS],
}

impl PendingCounters {
    fn record(&self, reason: PendingReason, kind: PendingKind) {
        self.total[reason_index(reason)][kind_index(kind)].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> [[u64; PENDING_KINDS]; PENDING_REASONS] {
        let mut out = [[0_u64; PENDING_KINDS]; PENDING_REASONS];
        for (reason_i, row) in out.iter_mut().enumerate() {
            for (kind_i, cell) in row.iter_mut().enumerate() {
                *cell = self.total[reason_i][kind_i].load(Ordering::Relaxed);
            }
        }
        out
    }
}

pub const RECONCILIATION_RESULTS: usize = ACK_RESULTS;

#[derive(Debug, Default)]
struct ReconciliationCounters {
    total: [AtomicU64; RECONCILIATION_RESULTS],
    wait_ms: [DurationHistogram; RECONCILIATION_RESULTS],
}

impl ReconciliationCounters {
    fn record(&self, result: AckResult, waited: bool, elapsed: Duration) {
        let result_i = result_index(result);
        self.total[result_i].fetch_add(1, Ordering::Relaxed);
        if waited && result.observed_wait() {
            self.wait_ms[result_i].observe_ms(elapsed.as_millis() as u64);
        }
    }

    #[allow(clippy::type_complexity)] // fixed-shape label arrays
    fn snapshot(
        &self,
    ) -> (
        [u64; RECONCILIATION_RESULTS],
        [[u64; BUCKET_COUNT]; RECONCILIATION_RESULTS],
    ) {
        let mut total = [0_u64; RECONCILIATION_RESULTS];
        let mut wait_ms = [[0_u64; BUCKET_COUNT]; RECONCILIATION_RESULTS];
        for (result_i, cell) in total.iter_mut().enumerate() {
            *cell = self.total[result_i].load(Ordering::Relaxed);
            wait_ms[result_i] = self.wait_ms[result_i].snapshot();
        }
        (total, wait_ms)
    }
}

/// Per-repository visibility telemetry. Attached to `RepoTxGate`, so every
/// recording site that already holds the gate reaches it lock-free.
#[derive(Debug)]
pub struct VisibilityMetrics {
    started: std::time::Instant,
    ack: AckCounters,
    pending: PendingCounters,
    read_wait_timeouts: AtomicU64,
    reconciliation: ReconciliationCounters,
    /// Rolling rate-limit reservation for pending-expiry warnings, mirroring
    /// the gate's own visibility-warning discipline (relaxed publishes only
    /// a deadline, never payload).
    next_pending_warning_ns: AtomicU64,
}

impl Default for VisibilityMetrics {
    fn default() -> Self {
        Self::new()
    }
}

const PENDING_WARNING_INTERVAL: Duration = Duration::from_secs(5);

impl VisibilityMetrics {
    pub fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            ack: AckCounters::default(),
            pending: PendingCounters::default(),
            read_wait_timeouts: AtomicU64::new(0),
            reconciliation: ReconciliationCounters::default(),
            next_pending_warning_ns: AtomicU64::new(0),
        }
    }

    /// Result counter always; duration histogram ONLY when the caller truly
    /// waited (`waited`) and the result carries a duration.
    pub fn record_ack(&self, mode: AckMode, result: AckResult, waited: bool, elapsed: Duration) {
        self.ack.record(mode, result, waited, elapsed);
    }

    pub fn record_pending(&self, reason: impl Into<PendingReason>, kind: PendingKind) {
        self.pending.record(reason.into(), kind);
    }

    pub fn record_read_wait_timeout(&self) {
        self.read_wait_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_reconciliation(&self, result: AckResult, waited: bool, elapsed: Duration) {
        self.reconciliation.record(result, waited, elapsed);
    }

    /// True at most once per [`PENDING_WARNING_INTERVAL`], so a stall cannot
    /// flood the structured log.
    pub fn reserve_pending_warning(&self) -> bool {
        let now = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.next_pending_warning_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                if now >= next && now != u64::MAX {
                    Some(now.saturating_add(PENDING_WARNING_INTERVAL.as_nanos() as u64))
                } else {
                    None
                }
            })
            .is_ok()
    }

    pub fn snapshot(&self) -> super::snapshot::VisibilityMetricsSnapshot {
        let (ack_total, ack_wait_ms) = self.ack.snapshot();
        let (reconciliation_total, reconciliation_wait_ms) = self.reconciliation.snapshot();
        super::snapshot::VisibilityMetricsSnapshot {
            ack_total,
            ack_wait_ms,
            pending_total: self.pending.snapshot(),
            read_wait_timeouts: self.read_wait_timeouts.load(Ordering::Relaxed),
            reconciliation_total,
            reconciliation_wait_ms,
        }
    }
}

pub fn mode_index(mode: AckMode) -> usize {
    match mode {
        AckMode::Strict => 0,
        AckMode::Bounded => 1,
    }
}

pub fn result_index(result: AckResult) -> usize {
    match result {
        AckResult::Visible => 0,
        AckResult::PendingTimeout => 1,
        AckResult::PendingReopen => 2,
        AckResult::Error => 3,
    }
}

pub fn reason_index(reason: impl Into<PendingReason>) -> usize {
    match reason.into() {
        PendingReason::Timeout => 0,
        PendingReason::ReopenRequired => 1,
    }
}

pub fn kind_index(kind: PendingKind) -> usize {
    match kind {
        PendingKind::ExplicitTx => 0,
        PendingKind::ImplicitBatch => 1,
        PendingKind::DirectWrite => 2,
    }
}

/// Shared mapping of a settled-prefix sample onto the metric label space.
pub fn ack_result_of(visibility: VisibilityState) -> AckResult {
    match visibility {
        VisibilityState::Visible => AckResult::Visible,
        VisibilityState::Pending {
            reason: VisibilityPendingReason::Timeout,
            ..
        } => AckResult::PendingTimeout,
        VisibilityState::Pending {
            reason: VisibilityPendingReason::ReopenRequired,
            ..
        } => AckResult::PendingReopen,
    }
}
