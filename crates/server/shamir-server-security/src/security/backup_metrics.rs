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
//! last run duration and the per-stage failure split; the writer-block
//! barrier durations (`phase=capture|seal`), the pin duration and the WAL
//! segment count have no observable source yet and stay unset.

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
    /// Duration of the last finished run; `None` until a run has finished.
    pub last_duration_seconds: Option<f64>,
    /// Writer-visibility barrier durations per phase (`capture` / `seal`);
    /// not observable through the current port adapter — always `None`
    /// this step (documented gap, §4.8 / release plan).
    pub writer_block_capture_seconds: Option<f64>,
    pub writer_block_seal_seconds: Option<f64>,
    /// Truncation-pin hold duration; not observable this step.
    pub pin_seconds: Option<f64>,
    /// WAL segments copied; the protocol's progress exposes only hashed
    /// bytes/files, so the segment count is not yet measured.
    pub wal_segments_copied_total: Option<u64>,
}

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
        "Duration of the last finished online backup run (unset until the \
         first run finishes)"
    );
    metrics::describe_gauge!(
        "shamir_backup_writer_block_seconds",
        metrics::Unit::Seconds,
        "Writer-visibility barrier duration per phase (capture|seal); not \
         emitted while the adapter cannot measure barrier durations"
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
}

/// Per-scrape rendering: read the owner's snapshot and raise the recorder.
pub(crate) fn record_backup_metrics() {
    let Some(source) = SOURCE.get() else {
        return;
    };
    let snap = source();

    metrics::counter!("shamir_backup_started_total", "kind" => "online")
        .absolute(snap.started_total);
    metrics::counter!("shamir_backup_completed_total", "kind" => "online")
        .absolute(snap.completed_total);
    for (stage, count) in snap.failed_total_by_stage {
        metrics::counter!(
            "shamir_backup_failed_total",
            "kind" => "online", "stage" => stage
        )
        .absolute(count);
    }
    metrics::gauge!("shamir_backup_active").set(u8::from(snap.active) as f64);
    metrics::gauge!("shamir_backup_bytes_copied").set(snap.bytes_copied_total as f64);
    if let Some(seconds) = snap.last_duration_seconds {
        metrics::gauge!("shamir_backup_duration_seconds").set(seconds);
    }
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
