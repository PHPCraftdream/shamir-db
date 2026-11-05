//! The bounded in-memory record ring shared by the `log` capture and, behind
//! the `tracing` feature, the tracing layer.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// Default maximum number of records kept (design: 2000).
pub const DEFAULT_RECORD_CAP: usize = 2000;

/// Default per-record line cap in characters; longer messages are truncated
/// so one huge log line cannot blow up the report.
pub const DEFAULT_LINE_CAP: usize = 2000;

const TRUNCATION_MARKER: &str = "...[truncated]";

/// One captured `log`/`tracing` record.
#[derive(Clone, Debug)]
pub struct RingRecord {
    /// Monotonic instant of the push; rendered relative to the reporting
    /// guard's start instant.
    pub at: Instant,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Bounded FIFO of records.
///
/// Contention model: TEST-ONLY diagnostics — the writers are the process's
/// `log`/`tracing` emitters during a single test (the test runner executes
/// one test per process, so the ring belongs to that test), the readers are
/// the reporting guard's `Drop` and the detached watchdog thread. Contention
/// is structurally bounded and there is no production path; this is the
/// sanctioned setup/test-fixture fallback for `std::sync::Mutex`. Every lock
/// access is poison-tolerant so the sink itself can never panic.
pub struct DiagRing {
    records: Mutex<VecDeque<RingRecord>>,
    record_cap: usize,
    line_cap: usize,
    dropped: AtomicUsize,
}

impl DiagRing {
    pub fn new(record_cap: usize, line_cap: usize) -> Self {
        Self {
            records: Mutex::new(VecDeque::new()),
            record_cap,
            line_cap,
            dropped: AtomicUsize::new(0),
        }
    }

    /// Pushes one record, truncating `message` to the line cap and evicting
    /// the oldest record once the ring is full. Never panics.
    pub fn push(
        &self,
        level: impl Into<String>,
        target: impl Into<String>,
        message: impl Into<String>,
    ) {
        if self.record_cap == 0 {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let record = RingRecord {
            at: Instant::now(),
            level: level.into(),
            target: truncate_chars(&target.into(), self.line_cap),
            message: truncate_chars(&message.into(), self.line_cap),
        };
        let mut records = self.lock();
        if records.len() >= self.record_cap {
            records.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        records.push_back(record);
    }

    /// Snapshot of the kept records, oldest first.
    pub fn snapshot(&self) -> Vec<RingRecord> {
        self.lock().iter().cloned().collect()
    }

    /// How many records were evicted by the bound so far.
    pub fn dropped_count(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The configured record capacity.
    pub fn record_cap(&self) -> usize {
        self.record_cap
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<RingRecord>> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The process-wide ring. The `log` capture and the `tracing` layer both
/// write here; guard reports read it.
pub fn shared_ring() -> &'static Arc<DiagRing> {
    static SHARED: OnceLock<Arc<DiagRing>> = OnceLock::new();
    SHARED.get_or_init(|| Arc::new(DiagRing::new(DEFAULT_RECORD_CAP, DEFAULT_LINE_CAP)))
}

/// Renders records oldest-first as `+<secs>s LEVEL target: message`, the
/// seconds relative to `base`.
pub fn render_records(records: &[RingRecord], base: Instant) -> String {
    let mut out = String::new();
    for record in records {
        let elapsed = record.at.saturating_duration_since(base).as_secs_f64();
        let _ = writeln!(
            out,
            "  +{elapsed:.3}s {} {}: {}",
            record.level, record.target, record.message
        );
    }
    out
}

fn truncate_chars(message: &str, cap: usize) -> String {
    if message.chars().count() <= cap {
        return message.to_string();
    }
    let mut out: String = message.chars().take(cap).collect();
    out.push_str(TRUNCATION_MARKER);
    out
}
