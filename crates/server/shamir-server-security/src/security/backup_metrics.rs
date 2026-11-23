//! Online-backup metric export (`online-backup.md` §4.8): zero-touch
//! registration and per-scrape snapshot rendering for the `shamir_backup_*`
//! family. The snapshot values are owned by the backup subsystem
//! (`shamir-server`'s online-backup registry), which installs one process-wide
//! source callback at boot — the same owner-installed-source pattern as
//! `export::replication`. This module owns all recorder access; the owner
//! never touches `metrics` directly.
//!
//! Truthfulness rule: a value the current backup step cannot observe is
//! `None` in the snapshot and its series is described (so dashboards see the
//! family) but never emitted with a fabricated value. This step measures
//! started/completed/failed counters, the active gauge, copied bytes, the
//! last successful run, scheduled outcomes, and retention. The writer-block
//! windows (`phase=capture|seal`) are the LAST completed close→reopen
//! window per phase as recorded by the engine (a last-window gauge, not an
//! aggregate, unset until the first barrier completes); the pin duration
//! and the WAL segment count have no observable source yet and stay unset.
//!
//! Four LAST-VALUE gauges (`SCRAPE_RENDERED_GAUGES`) are NOT stored in the
//! shared recorder: `render_backup_scrape_gauges` appends them to the
//! `/metrics` body from the scrape's OWN snapshot, like the per-server
//! activity gauges (`export::activity`). A `gauge!(..).set(..)` is a plain
//! last-writer-wins store, so two overlapping scrapes could otherwise apply in
//! the reverse of their read order and an older scrape would republish an
//! older value. Rendering from the snapshot makes each response internally
//! consistent and independent of every other scrape.

use std::sync::OnceLock;

/// Closed label set for a backup run stage (failure-stage label source).
pub const BACKUP_STAGES: [&str; 7] = [
    "preparing",
    "captured",
    "copying",
    "sealing",
    "verifying",
    "done",
    "failed",
];

const SCHEDULED_RESULTS: [&str; 5] = [
    "started",
    "skipped_busy",
    "skipped_window",
    "failed",
    "completed",
];

/// Immutable backup values supplied by the owning subsystem at scrape.
/// Cumulative counters must be monotone for the process lifetime (the
/// recorder renders them with `absolute`, so overlapping scrapes never
/// regress them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackupMetricsSnapshot {
    pub started_total: u64,
    pub completed_total: u64,
    /// Cumulative failures per failure stage label (`stage` label values
    /// are the lowercase stage names; see [`BACKUP_STAGES`]).
    pub failed_total_by_stage: Vec<(String, u64)>,
    pub active: bool,
    /// Total bytes copied by finished runs (the protocol's per-repo hashed
    /// byte counters, summed).
    pub bytes_copied_total: u64,
    /// Legacy millisecond-resolution duration of the last successful run.
    pub last_duration_seconds: Option<f64>,
    pub last_success_timestamp_seconds: u64,
    pub last_success_duration_seconds: f64,
    pub last_bytes: u64,
    /// Ordered as started, skipped_busy, skipped_window, failed, completed.
    pub scheduled_runs_total: [u64; SCHEDULED_RESULTS.len()],
    pub scheduled_failure_streak: u64,
    pub schedule_enabled: bool,
    /// Unix seconds when scheduling started, or zero before enablement.
    pub schedule_started_timestamp_seconds: u64,
    pub retention_deleted_total: u64,
    pub retention_failed_total: u64,
    pub retention_untouched: u64,
    /// Writer-visibility barrier durations per phase (`capture` / `seal`):
    /// the LAST completed close→reopen window per phase as recorded by the
    /// engine (last-window gauge; each completed barrier overwrites its
    /// phase after the gate reopened — success, error, timeout,
    /// cancellation or unwind). `None` until the first window of that
    /// phase completes in this process.
    pub writer_block_capture_seconds: Option<f64>,
    pub writer_block_seal_seconds: Option<f64>,
    /// Truncation-pin hold duration; not observable this step.
    pub pin_seconds: Option<f64>,
    /// WAL segments copied; the protocol's progress exposes only hashed
    /// bytes/files, so the segment count is not yet measured.
    pub wal_segments_copied_total: Option<u64>,
}

/// The four LAST-VALUE gauges rendered per scrape by
/// [`render_backup_scrape_gauges`]; they are never written to the shared
/// recorder (see the module doc). Their `describe_gauge!` help text stays in
/// [`register_backup_metrics`].
pub(crate) const SCRAPE_RENDERED_GAUGES: [&str; 4] = [
    "shamir_backup_last_success_timestamp_seconds",
    "shamir_backup_scheduled_failure_streak",
    "shamir_backup_last_duration_seconds",
    "shamir_backup_duration_seconds",
];

static SOURCE: OnceLock<fn() -> BackupMetricsSnapshot> = OnceLock::new();

/// Install the process-wide snapshot source. The first owner registration
/// wins (one online backup per server, so one source is enough).
pub fn install_backup_metrics_source(source: fn() -> BackupMetricsSnapshot) {
    let _ = SOURCE.set(source);
}

/// Describe every series so the first scrape sees the whole family, even
/// before the first backup runs.
pub(crate) fn register_backup_metrics() {
    metrics::describe_counter!(
        "shamir_backup_started_total",
        metrics::Unit::Count,
        "Online backup runs started (label kind=online)"
    );
    metrics::describe_counter!(
        "shamir_backup_completed_total",
        metrics::Unit::Count,
        "Online backup runs finished successfully (kind=online)"
    );
    metrics::describe_counter!(
        "shamir_backup_failed_total",
        metrics::Unit::Count,
        "Online backup runs failed, split by the stage of the failure \
         (kind=online, stage=preparing|captured|copying|sealing|verifying)"
    );
    metrics::describe_gauge!(
        "shamir_backup_active",
        metrics::Unit::Count,
        "1 while an online backup run is in flight, 0 otherwise"
    );
    metrics::describe_gauge!(
        "shamir_backup_bytes_copied",
        metrics::Unit::Bytes,
        "Cumulative bytes copied by finished online backup runs"
    );
    metrics::describe_gauge!(
        "shamir_backup_duration_seconds",
        metrics::Unit::Seconds,
        "Duration of the last successful online backup run (unset until the \
         first success)"
    );
    metrics::describe_gauge!(
        "shamir_backup_writer_block_seconds",
        metrics::Unit::Seconds,
        "Last completed writer-visibility (close-to-reopen) window per phase \
         (capture|seal); a last-window gauge, not an aggregate — unset until \
         the first barrier of that phase completes"
    );
    metrics::describe_gauge!(
        "shamir_backup_pin_seconds",
        metrics::Unit::Seconds,
        "Truncation-pin hold duration; not emitted while unmeasured"
    );
    metrics::describe_counter!(
        "shamir_backup_wal_segments_copied_total",
        metrics::Unit::Count,
        "WAL segments copied; not emitted while unmeasured"
    );

    metrics::describe_gauge!(
        "shamir_backup_last_success_timestamp_seconds",
        metrics::Unit::Seconds,
        "Unix time of the last successful online backup, or zero before success"
    );
    metrics::describe_gauge!(
        "shamir_backup_last_duration_seconds",
        metrics::Unit::Seconds,
        "Duration of the last successful online backup"
    );
    metrics::describe_gauge!(
        "shamir_backup_last_bytes",
        metrics::Unit::Bytes,
        "Manifest payload bytes of the last successful online backup"
    );
    metrics::describe_counter!(
        "shamir_backup_scheduled_runs_total",
        metrics::Unit::Count,
        "Scheduled online backup events by result"
    );
    metrics::describe_gauge!(
        "shamir_backup_scheduled_failure_streak",
        metrics::Unit::Count,
        "Consecutive scheduled backup failures, ignoring skips"
    );
    metrics::describe_gauge!(
        "shamir_backup_schedule_enabled",
        metrics::Unit::Count,
        "1 when the online backup schedule is enabled, 0 otherwise"
    );
    metrics::describe_gauge!(
        "shamir_backup_schedule_started_timestamp_seconds",
        metrics::Unit::Seconds,
        "Unix time when the online backup schedule started, or zero before enablement"
    );
    metrics::describe_counter!(
        "shamir_backup_retention_deleted_total",
        metrics::Unit::Count,
        "Backup directories successfully deleted by retention"
    );
    metrics::describe_counter!(
        "shamir_backup_retention_failed_total",
        metrics::Unit::Count,
        "Backup retention errors"
    );
    metrics::describe_gauge!(
        "shamir_backup_retention_untouched",
        metrics::Unit::Count,
        "Foreign or unverifiable entries in the last complete retention classification"
    );

    metrics::counter!("shamir_backup_started_total", "kind" => "online").increment(0);
    metrics::counter!("shamir_backup_completed_total", "kind" => "online").increment(0);
    for stage in BACKUP_STAGES {
        metrics::counter!(
            "shamir_backup_failed_total",
            "kind" => "online", "stage" => stage
        )
        .increment(0);
    }
    metrics::gauge!("shamir_backup_active").set(0.0);
    metrics::gauge!("shamir_backup_bytes_copied").set(0.0);
    record_schedule_metrics(&BackupMetricsSnapshot::default());
}

/// The scheduled/retention subset that STAYS in the shared recorder. The
/// LAST-VALUE gauges of [`SCRAPE_RENDERED_GAUGES`] are deliberately absent:
/// the recorder path would emit a second, last-writer-wins copy the response
/// body also carries (duplicate series) — they are rendered per scrape by
/// [`render_backup_scrape_gauges`] instead.
fn record_schedule_metrics(snap: &BackupMetricsSnapshot) {
    metrics::gauge!("shamir_backup_last_bytes").set(snap.last_bytes as f64);
    for (result, count) in SCHEDULED_RESULTS.into_iter().zip(snap.scheduled_runs_total) {
        metrics::counter!("shamir_backup_scheduled_runs_total", "result" => result).absolute(count);
    }
    metrics::gauge!("shamir_backup_schedule_enabled")
        .set(f64::from(u8::from(snap.schedule_enabled)));
    metrics::gauge!("shamir_backup_schedule_started_timestamp_seconds")
        .set(snap.schedule_started_timestamp_seconds as f64);
    metrics::counter!("shamir_backup_retention_deleted_total")
        .absolute(snap.retention_deleted_total);
    metrics::counter!("shamir_backup_retention_failed_total").absolute(snap.retention_failed_total);
    metrics::gauge!("shamir_backup_retention_untouched").set(snap.retention_untouched as f64);
}

/// Read the owner-installed snapshot for THIS scrape. `None` while no owner is
/// installed (a process that never booted a server, or a test harness) — the
/// caller then renders and records nothing for the backup family.
pub(crate) fn backup_scrape_snapshot() -> Option<BackupMetricsSnapshot> {
    let source = SOURCE.get().copied()?;
    Some(source())
}

/// Per-scrape recorder update of every backup series EXCEPT the four
/// LAST-VALUE gauges of [`SCRAPE_RENDERED_GAUGES`], which
/// [`render_backup_scrape_gauges`] renders from this same snapshot straight
/// into the response body.
pub(crate) fn record_backup_metrics(snap: &BackupMetricsSnapshot) {
    record_schedule_metrics(snap);

    metrics::counter!("shamir_backup_started_total", "kind" => "online")
        .absolute(snap.started_total);
    metrics::counter!("shamir_backup_completed_total", "kind" => "online")
        .absolute(snap.completed_total);
    for (stage, count) in &snap.failed_total_by_stage {
        metrics::counter!(
            "shamir_backup_failed_total",
            "kind" => "online", "stage" => stage.clone()
        )
        .absolute(*count);
    }
    metrics::gauge!("shamir_backup_active").set(u8::from(snap.active) as f64);
    metrics::gauge!("shamir_backup_bytes_copied").set(snap.bytes_copied_total as f64);
    // Observable-only emission: a barrier/pin/segment value the backup step
    // cannot measure is never rendered as a fabricated zero.
    if let Some(seconds) = snap.writer_block_capture_seconds {
        metrics::gauge!("shamir_backup_writer_block_seconds", "phase" => "capture").set(seconds);
    }
    if let Some(seconds) = snap.writer_block_seal_seconds {
        metrics::gauge!("shamir_backup_writer_block_seconds", "phase" => "seal").set(seconds);
    }
    if let Some(seconds) = snap.pin_seconds {
        metrics::gauge!("shamir_backup_pin_seconds").set(seconds);
    }
    if let Some(segments) = snap.wal_segments_copied_total {
        metrics::counter!("shamir_backup_wal_segments_copied_total").absolute(segments);
    }
}

/// Render the four LAST-VALUE gauges of [`SCRAPE_RENDERED_GAUGES`] in
/// Prometheus text format from THIS scrape's snapshot, ready to append after
/// the global recorder's render output (the `render_activity_series`
/// precedent). Exactly one `# TYPE` line per emitted series; nothing here
/// touches the shared recorder, so no second copy can exist.
///
/// `shamir_backup_duration_seconds` is observable-only, the same rule the
/// recorder path used: it is omitted until the first successful copy of the
/// process supplies a duration (`last_duration_seconds` is `None`).
pub(crate) fn render_backup_scrape_gauges(snap: &BackupMetricsSnapshot) -> String {
    let [last_success, streak, last_duration, duration] = SCRAPE_RENDERED_GAUGES;
    let mut out = String::new();
    push_scrape_gauge(&mut out, last_success, snap.last_success_timestamp_seconds);
    push_scrape_gauge(&mut out, streak, snap.scheduled_failure_streak);
    push_scrape_gauge(&mut out, last_duration, snap.last_success_duration_seconds);
    if let Some(seconds) = snap.last_duration_seconds {
        push_scrape_gauge(&mut out, duration, seconds);
    }
    out
}

/// One typed unlabeled gauge series: the `# TYPE` line then the sample line.
fn push_scrape_gauge(out: &mut String, name: &str, value: impl std::fmt::Display) {
    out.push_str(&format!("# TYPE {name} gauge\n{name} {value}\n"));
}
