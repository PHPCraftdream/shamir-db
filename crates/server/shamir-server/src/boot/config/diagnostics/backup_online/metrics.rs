//! Owner-side `shamir_backup_*` metric state (design `online-backup.md`
//! §4.8, R2a step 3a). The registry notes run events here; scrape time the
//! process-wide source installed into shamir-server-security's observability
//! render path copies the atomics into a `BackupMetricsSnapshot`. Counters
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
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

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

struct BackupMetricsState {
    started: AtomicU64,
    completed: AtomicU64,
    failed: [AtomicU64; STAGE_LABELS.len()],
    active: AtomicBool,
    bytes_copied: AtomicU64,
    has_duration: AtomicBool,
    last_duration_ms: AtomicU64,
}

impl BackupMetricsState {
    const fn new() -> Self {
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
            has_duration: AtomicBool::new(false),
            last_duration_ms: AtomicU64::new(0),
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
    let (capture_window, seal_window) = RepoInstance::online_backup_writer_block_seconds();
    BackupMetricsSnapshot {
        started_total: state.started.load(Ordering::Relaxed),
        completed_total: state.completed.load(Ordering::Relaxed),
        failed_total_by_stage,
        active: state.active.load(Ordering::Relaxed),
        bytes_copied_total: state.bytes_copied.load(Ordering::Relaxed),
        last_duration_seconds: state
            .has_duration
            .load(Ordering::Relaxed)
            .then(|| state.last_duration_ms.load(Ordering::Relaxed) as f64 / 1_000.0),
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

/// Note a run end: the final stage, whether it failed, the run's copied
/// bytes (the protocol's hashed-byte counters) and its wall duration.
pub fn note_finished(failed: bool, final_stage: BackupStage, bytes: u64, duration: Duration) {
    let state = &*STATE;
    state.active.store(false, Ordering::Relaxed);
    if failed {
        state.failed[stage_index(final_stage)].fetch_add(1, Ordering::Relaxed);
    } else {
        state.completed.fetch_add(1, Ordering::Relaxed);
        state.bytes_copied.fetch_add(bytes, Ordering::Relaxed);
        state.has_duration.store(true, Ordering::Relaxed);
        state.last_duration_ms.store(
            duration.as_millis().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
    }
}
