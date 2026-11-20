//! Owner-side `shamir_backup_*` metric state (`online-backup.md` §4.8 and
//! `backup-schedule.md`). The registry and scheduler note events here; the
//! process-wide source supplies a `BackupMetricsSnapshot` at scrape time. Counters
//! are process-lifetime cumulative so the recorder's `absolute` rendering
//! never regresses them across overlapping scrapes.
//!
//! Writer-block windows come from the engine's per-phase last-window cells
//! (`RepoInstance::online_backup_writer_block_seconds`, process-local): each
//! completed capture or seal barrier — success, error, timeout, cancellation
//! or unwind — overwrites its phase cell exactly once after the gate has
//! reopened, so the exported gauge is the LAST completed window per phase
//! (`phase=capture|seal`), not an aggregate. Truncation-pin hold duration
//! and the WAL segment count are still unmeasured: no observable source
//! exists, so they stay `None` in the snapshot — the renderer emits nothing
//! for them rather than a fabricated zero. `BackupProgress` carries only
//! hashed bytes/files.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwapOption;

use shamir_db::engine::repo::RepoInstance;
use shamir_server_security::observability::{install_backup_metrics_source, BackupMetricsSnapshot};

use crate::sync::durability::backup::online::progress::BackupStage;

/// Lowercase stage labels, mirroring the `BackupStage` serde naming; index
/// order matches the variant order in [`BackupStage`].
const STAGE_LABELS: [&str; 7] = [
    "preparing",
    "captured",
    "copying",
    "sealing",
    "verifying",
    "done",
    "failed",
];

fn stage_index(stage: BackupStage) -> usize {
    match stage {
        BackupStage::Preparing => 0,
        BackupStage::Captured => 1,
        BackupStage::Copying => 2,
        BackupStage::Sealing => 3,
        BackupStage::Verifying => 4,
        BackupStage::Done => 5,
        BackupStage::Failed => 6,
    }
}

/// Variant order matches the exporter’s fixed `result` label order.
#[derive(Clone, Copy)]
pub(super) enum ScheduledResult {
    Started,
    SkippedBusy,
    SkippedWindow,
    Failed,
    Completed,
}

struct SuccessfulBackup {
    timestamp_seconds: u64,
    duration: Duration,
    bytes: u64,
}

struct BackupMetricsState {
    started: AtomicU64,
    completed: AtomicU64,
    failed: [AtomicU64; STAGE_LABELS.len()],
    active: AtomicBool,
    bytes_copied: AtomicU64,
    last_success: ArcSwapOption<SuccessfulBackup>,
    scheduled_runs: [AtomicU64; 5],
    scheduled_failure_streak: AtomicU64,
    schedule_started_timestamp_seconds: AtomicU64,
    retention_deleted: AtomicU64,
    retention_failed: AtomicU64,
    retention_untouched: AtomicU64,
}

impl BackupMetricsState {
    fn new() -> Self {
        Self {
            started: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            failed: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            active: AtomicBool::new(false),
            bytes_copied: AtomicU64::new(0),
            last_success: ArcSwapOption::empty(),
            scheduled_runs: std::array::from_fn(|_| AtomicU64::new(0)),
            scheduled_failure_streak: AtomicU64::new(0),
            schedule_started_timestamp_seconds: AtomicU64::new(0),
            retention_deleted: AtomicU64::new(0),
            retention_failed: AtomicU64::new(0),
            retention_untouched: AtomicU64::new(0),
        }
    }
}

static STATE: LazyLock<BackupMetricsState> = LazyLock::new(BackupMetricsState::new);
static SOURCE_INSTALLED: OnceLock<()> = OnceLock::new();

/// Scrape-time snapshot from the process-lifetime counters.
pub fn backup_metrics_snapshot() -> BackupMetricsSnapshot {
    let state = &*STATE;
    let failed_total_by_stage = STAGE_LABELS
        .iter()
        .zip(state.failed.iter())
        .map(|(label, count)| ((*label).to_owned(), count.load(Ordering::Relaxed)))
        .filter(|(_, count)| *count > 0)
        .collect();
    let last_success = state.last_success.load();
    let last_success = last_success.as_deref();
    let scheduled_failure_streak = state.scheduled_failure_streak.load(Ordering::Acquire);
    let schedule_started_timestamp_seconds = state
        .schedule_started_timestamp_seconds
        .load(Ordering::Relaxed);
    let (capture_window, seal_window) = RepoInstance::online_backup_writer_block_seconds();
    BackupMetricsSnapshot {
        started_total: state.started.load(Ordering::Relaxed),
        completed_total: state.completed.load(Ordering::Relaxed),
        failed_total_by_stage,
        active: state.active.load(Ordering::Relaxed),
        bytes_copied_total: state.bytes_copied.load(Ordering::Relaxed),
        last_duration_seconds: last_success
            .map(|success| success.duration.as_millis().min(u128::from(u64::MAX)) as f64 / 1_000.0),
        last_success_timestamp_seconds: last_success.map_or(0, |success| success.timestamp_seconds),
        last_success_duration_seconds: last_success
            .map_or(0.0, |success| success.duration.as_secs_f64()),
        last_bytes: last_success.map_or(0, |success| success.bytes),
        scheduled_runs_total: std::array::from_fn(|i| {
            state.scheduled_runs[i].load(Ordering::Relaxed)
        }),
        scheduled_failure_streak,
        schedule_enabled: schedule_started_timestamp_seconds != 0,
        schedule_started_timestamp_seconds,
        retention_deleted_total: state.retention_deleted.load(Ordering::Relaxed),
        retention_failed_total: state.retention_failed.load(Ordering::Relaxed),
        retention_untouched: state.retention_untouched.load(Ordering::Relaxed),
        writer_block_capture_seconds: capture_window,
        writer_block_seal_seconds: seal_window,
        pin_seconds: None,
        wal_segments_copied_total: None,
    }
}

fn install_snapshot_source() {
    SOURCE_INSTALLED.get_or_init(|| install_backup_metrics_source(backup_metrics_snapshot));
}

/// Note a run start (registry `start` after the runner thread spawned).
pub fn note_started() {
    install_snapshot_source();
    let state = &*STATE;
    state.started.fetch_add(1, Ordering::Relaxed);
    state.active.store(true, Ordering::Relaxed);
}

/// Account for the run outcome and cumulative per-repo hashed bytes on success.
/// The registry publishes the successful report and duration through `note_success`.
pub fn note_finished(failed: bool, final_stage: BackupStage, bytes: u64, _duration: Duration) {
    let state = &*STATE;
    state.active.store(false, Ordering::Relaxed);
    if failed {
        state.failed[stage_index(final_stage)].fetch_add(1, Ordering::Relaxed);
    } else {
        state.completed.fetch_add(1, Ordering::Relaxed);
        state.bytes_copied.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Publish a successful protocol report (admin or scheduled), before releasing
/// the run permit. `bytes` is the full manifest payload, not the repo counter.
pub(super) fn note_success(bytes: u64, duration: Duration) {
    let timestamp_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    STATE.last_success.store(Some(Arc::new(SuccessfulBackup {
        timestamp_seconds,
        duration,
        bytes,
    })));
}

pub(super) fn enable_schedule() {
    // Reserve zero for disabled, even if the system clock is at or before the epoch.
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.as_secs().max(1));
    STATE
        .schedule_started_timestamp_seconds
        .store(started, Ordering::Relaxed);
    install_snapshot_source();
}

pub(super) fn note_scheduled(result: ScheduledResult) {
    let state = &*STATE;
    saturating_add(&state.scheduled_runs[result as usize], 1, Ordering::Relaxed);
    match result {
        ScheduledResult::Failed => {
            // Publish the failed counter before the corresponding streak.
            saturating_add(&state.scheduled_failure_streak, 1, Ordering::Release);
        }
        ScheduledResult::Completed => state.scheduled_failure_streak.store(0, Ordering::Release),
        ScheduledResult::Started
        | ScheduledResult::SkippedBusy
        | ScheduledResult::SkippedWindow => {}
    }
}

/// `None` leaves the last complete classification intact.
pub(super) fn note_retention(deleted: u64, failed: u64, untouched: Option<u64>) {
    let state = &*STATE;
    saturating_add(&state.retention_deleted, deleted, Ordering::Relaxed);
    saturating_add(&state.retention_failed, failed, Ordering::Relaxed);
    if let Some(count) = untouched {
        state.retention_untouched.store(count, Ordering::Relaxed);
    }
}

fn saturating_add(counter: &AtomicU64, amount: u64, order: Ordering) {
    let _ = counter.fetch_update(order, Ordering::Relaxed, |count| {
        Some(count.saturating_add(amount))
    });
}
