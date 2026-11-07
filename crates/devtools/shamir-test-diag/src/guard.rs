//! [`FlakeGuard`] — the test-scoped timeline, notes and failure-only report.

use crate::log_capture::{install_log_ring, LogCaptureStatus};
use crate::ring::{render_records, shared_ring};
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Where reports are written. `None` = stderr (the design's default).
type ReportSink = Arc<dyn Fn(&str) + Send + Sync>;

static REPORT_SINK: Mutex<Option<ReportSink>> = Mutex::new(None);

fn report_sink_slot() -> MutexGuard<'static, Option<ReportSink>> {
    REPORT_SINK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Test hook replacing stderr with an in-memory sink so this crate's unit
/// tests can assert what a green or panicking run prints.
#[cfg(test)]
pub(crate) fn set_report_sink(sink: Option<ReportSink>) {
    *report_sink_slot() = sink;
}

fn emit(text: &str) {
    // Clone the sink out before calling it so a panicking sink cannot poison
    // the slot lock.
    let sink = report_sink_slot().clone();
    match sink {
        Some(sink) => sink(text),
        // A failed stderr write must never panic (this runs while unwinding
        // and on the watchdog thread): the diagnostics are simply lost.
        None => {
            let _ = std::io::stderr().write_all(text.as_bytes());
        }
    }
}

/// One timeline entry (`mark`).
struct TimelineEntry {
    at: Duration,
    label: String,
}

/// One state fact (`note`).
struct NoteEntry {
    at: Duration,
    key: String,
    value: String,
}

/// The diagnostic guard: create at the top of a test, keep alive to its end.
///
/// The guard never changes a test's assertions or timing budget (design
/// rule): `mark`/`note` only append to in-memory vectors, and `Drop` prints
/// nothing on a green run — the report is emitted ONLY while the test is
/// panicking. The first guard in the process installs the `log` ring
/// capture; when a logger is already installed it degrades gracefully and
/// the report header says so.
pub struct FlakeGuard {
    shared: Arc<GuardShared>,
}

/// State shared with the detached watchdog thread. The watchdog holds ONLY
/// this diagnostic state (never the guard itself, never test state).
struct GuardShared {
    name: String,
    started: Instant,
    timeline: Mutex<Vec<TimelineEntry>>,
    notes: Mutex<Vec<NoteEntry>>,
    log_capture: LogCaptureStatus,
    watchdog_fired: AtomicBool,
    watchdog_fired_at_ms: AtomicU64,
}

impl FlakeGuard {
    /// Guard with no watchdog.
    pub fn new(name: impl Into<String>) -> Self {
        Self::build(name, None)
    }

    /// Guard with a watchdog that dumps the report ONCE if the guard is
    /// still alive after `after` (choose below the runner's slow-timeout
    /// kill so the dump lands before it), then keeps the test running.
    pub fn with_watchdog(name: impl Into<String>, after: Duration) -> Self {
        Self::build(name, Some(after))
    }

    fn build(name: impl Into<String>, watchdog_after: Option<Duration>) -> Self {
        let log_capture = install_log_ring();
        let shared = Arc::new(GuardShared {
            name: name.into(),
            started: Instant::now(),
            timeline: Mutex::new(Vec::new()),
            notes: Mutex::new(Vec::new()),
            log_capture,
            watchdog_fired: AtomicBool::new(false),
            watchdog_fired_at_ms: AtomicU64::new(0),
        });
        if let Some(after) = watchdog_after {
            let weak = Arc::downgrade(&shared);
            // Detached std thread: the join handle is dropped, the thread
            // holds only the shared diagnostic state, a sleeping detached
            // thread never prevents process exit, and every step below is
            // panic-free.
            std::thread::spawn(move || {
                std::thread::sleep(after);
                // Upgrade fails once the test finished and dropped its guard:
                // a finished test must not be reported.
                if let Some(shared) = weak.upgrade() {
                    let at = shared.started.elapsed().as_millis() as u64;
                    shared.watchdog_fired_at_ms.store(at, Ordering::SeqCst);
                    // Exactly-once dump, ordered after the timestamp store so
                    // any report that observes the flag reads the timestamp.
                    if !shared.watchdog_fired.swap(true, Ordering::SeqCst) {
                        emit(&shared.render_report(true));
                    }
                }
            });
        }
        Self { shared }
    }

    /// Appends one step to the wall-clock timeline.
    pub fn mark(&self, label: impl Into<String>) {
        let entry = TimelineEntry {
            at: self.shared.started.elapsed(),
            label: label.into(),
        };
        self.shared.timeline_slot().push(entry);
    }

    /// Records one state fact (ports, ids, counters, codes, the last frame).
    pub fn note(&self, key: impl Into<String>, value: impl Into<String>) {
        let entry = NoteEntry {
            at: self.shared.started.elapsed(),
            key: key.into(),
            value: value.into(),
        };
        self.shared.notes_slot().push(entry);
    }
}

impl Drop for FlakeGuard {
    fn drop(&mut self) {
        // Failure-only (design): the report prints ONLY while the test is
        // unwinding; a green run costs nothing on this path.
        if std::thread::panicking() {
            emit(&self.shared.render_report(false));
        }
    }
}

impl GuardShared {
    fn timeline_slot(&self) -> MutexGuard<'_, Vec<TimelineEntry>> {
        self.timeline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn notes_slot(&self) -> MutexGuard<'_, Vec<NoteEntry>> {
        self.notes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn render_report(&self, watchdog: bool) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "=== flake-diag: {} ===", self.name);
        let _ = writeln!(out, "elapsed: {:.3}s", self.started.elapsed().as_secs_f64());
        let _ = writeln!(
            out,
            "log-capture: {}",
            match self.log_capture {
                LogCaptureStatus::Installed => "installed".to_string(),
                LogCaptureStatus::Degraded => {
                    "degraded (another logger already owned the process logger)".to_string()
                }
            },
        );
        if watchdog {
            let at = self.watchdog_fired_at_ms.load(Ordering::SeqCst) as f64 / 1000.0;
            let _ = writeln!(
                out,
                "watchdog: FIRED at +{at:.3}s (guard still alive; dump before the runner kill)"
            );
        }
        let _ = writeln!(out, "--- timeline ---");
        for entry in self.timeline_slot().iter() {
            let _ = writeln!(out, "  +{:.3}s {}", entry.at.as_secs_f64(), entry.label);
        }
        let _ = writeln!(out, "--- notes ---");
        for entry in self.notes_slot().iter() {
            let _ = writeln!(
                out,
                "  +{:.3}s {}: {}",
                entry.at.as_secs_f64(),
                entry.key,
                entry.value
            );
        }
        if !watchdog && self.watchdog_fired.load(Ordering::SeqCst) {
            let _ = writeln!(out, "watchdog: already fired earlier (see the dump above)");
        }
        let ring = shared_ring();
        let records = ring.snapshot();
        let _ = writeln!(
            out,
            "--- log ring: {} records (cap {}, dropped {}), oldest first ---",
            records.len(),
            ring.record_cap(),
            ring.dropped_count(),
        );
        out.push_str(&render_records(&records, self.started));
        out
    }
}
