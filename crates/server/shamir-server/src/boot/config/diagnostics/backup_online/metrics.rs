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
use std::sync::{Arc, LazyLock, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwapOption;

use shamir_db::engine::repo::RepoInstance;
use shamir_server_security::observability::{install_backup_metrics_source, BackupMetricsSnapshot};

use crate::server_meta::ServerMetaStore;
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

/// In-memory detail of the newest success. Duration and bytes stay here; the
/// exported stamp lives in [`BackupMetricsState::export_last_success_seconds`],
/// which also carries the durable/armed-at evidence this value cannot.
struct SuccessfulBackup {
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
    /// THE exported last-success stamp (epoch seconds, `0` = none), with
    /// clamp-once semantics: boot seeds it with the newest of the durable
    /// `backup_armed_at_unix_secs_v1` and `backup_last_success_unix_secs_v1`
    /// rows, `note_success` raises it with `fetch_max`, and each scrape
    /// `fetch_min`s it to "now" IN PLACE — so a future stamp is replaced by
    /// the current time exactly once and later scrapes report that same
    /// clamped value while the exported age grows. Duration and bytes stay in
    /// `last_success`; the durable rows are never rewritten by the clamp.
    export_last_success_seconds: AtomicU64,
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
            export_last_success_seconds: AtomicU64::new(0),
            retention_deleted: AtomicU64::new(0),
            retention_failed: AtomicU64::new(0),
            retention_untouched: AtomicU64::new(0),
        }
    }
}

static STATE: LazyLock<BackupMetricsState> = LazyLock::new(BackupMetricsState::new);
static SOURCE_INSTALLED: OnceLock<()> = OnceLock::new();

/// Process-wide durable meta handle used to persist the last-success stamp.
///
/// Deliberately a `Weak`: a strong `Arc` in a `static` would never drop, so the
/// fjall file lock on `server_meta` could not be released by
/// `ServerHandle::shutdown` and a same-data-dir restart (every in-process
/// relaunch test) would fail to reopen the store. While the server runs the
/// boot-owned `Arc` keeps the upgrade alive; after shutdown the upgrade fails
/// and the write is simply skipped.
///
/// REPLACEABLE (an `ArcSwapOption`, not the former first-wins `OnceLock`):
/// every launch installs its own store, so a relaunch in the same process
/// persists through the NEW store instead of silently keeping the previous
/// (already closed) one.
static META_STORE: ArcSwapOption<Weak<ServerMetaStore>> = ArcSwapOption::const_empty();

/// Scrape-time snapshot from the process-lifetime counters.
///
/// The four LAST-VALUE gauges derived here
/// (`last_success_timestamp_seconds`, `scheduled_failure_streak`,
/// `last_duration_seconds`, `duration_seconds`) are NOT stored in the
/// process-global Prometheus recorder any more. Each `/metrics` request takes
/// one snapshot (`backup_scrape_snapshot`) and
/// `render_backup_scrape_gauges` (`shamir-server-security/src/security/backup_metrics.rs`)
/// renders them from THAT snapshot straight into its own response body, like
/// the per-server activity gauges. A scrape therefore never reads a value
/// another scrape wrote, and two overlapping scrapes cannot apply in the
/// reverse of their read order: an older concurrent scrape keeps its own older
/// value in its own body and cannot publish it over a newer one. The cumulative
/// counters remain immune by construction: they go through the recorder's
/// `.absolute()` (`CounterFn::absolute` → `AtomicU64::fetch_max`) and the owner
/// counters are monotone. A store-max guard would be wrong even so: a streak
/// legitimately drops to zero on success and `retention_untouched` legitimately
/// falls when a complete scan finds fewer unverifiable entries.
///
/// The remaining last-value gauges of this family (`active`, `bytes_copied`,
/// `last_bytes`, `schedule_enabled`, `schedule_started_timestamp_seconds`,
/// `retention_untouched`) still go through the recorder's `set`; they are
/// outside this change.
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
    // Clamp ONCE, in the cell: `fetch_min` writes "now" back, so a future
    // stamp (a forward clock step at success) is replaced by the current time
    // exactly once and later scrapes report that SAME value, letting the
    // exported age grow. `fetch_min` returns the PREVIOUS value, hence the
    // trailing `min`: the scrape that performs the write-back must not hand
    // out the future stamp it just clamped.
    let now = unix_seconds_now();
    let last_success_timestamp_seconds = state
        .export_last_success_seconds
        .fetch_min(now, Ordering::AcqRel)
        .min(now);
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
        last_success_timestamp_seconds,
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

/// Install this boot's durable staleness evidence.
///
/// Replaces any previous launch's handle (`META_STORE` documents why the
/// weak handle is replaceable). Arms the schedule clock only when a schedule
/// is configured, and only while the armed-at row is absent — a restart that
/// reaches no tick must keep the FIRST arming time — and removes it when no
/// schedule is configured, so re-enabling later starts a fresh clock. Seeds
/// `export_last_success_seconds` from the NEWEST of the armed-at row and the
/// last-success row, so a boot that reaches no tick still exports real
/// evidence, and installs the scrape source so the gauge exists before any
/// run.
///
/// Every store failure here is best-effort: a broken data dir logs one bounded
/// token (`persist-armed-at-failed`), never a boot failure and never a path.
pub fn install_backup_meta_store(store: Arc<ServerMetaStore>, schedule_configured: bool) {
    META_STORE.store(Some(Arc::new(Arc::downgrade(&store))));
    let armed_at = if schedule_configured {
        arm_schedule_clock(&store)
    } else {
        disarm_schedule_clock(&store)
    };
    // Best-effort: a read failure seeds "no evidence", never a boot failure.
    let last_success = store
        .backup_last_success_unix_ns()
        .ok()
        .flatten()
        .unwrap_or(0);
    STATE
        .export_last_success_seconds
        .store(armed_at.max(last_success), Ordering::Release);
    install_snapshot_source();
}

/// Persist the schedule's FIRST arming stamp, or keep the existing one.
fn arm_schedule_clock(store: &ServerMetaStore) -> u64 {
    if let Ok(Some(armed_at)) = store.backup_armed_at_unix_secs() {
        return armed_at;
    }
    // Absent (or unreadable — "unknown" is treated as absent) arms now; every
    // later boot sees the row and keeps this first stamp.
    let now = unix_seconds_now();
    if store.store_backup_armed_at_unix_secs(now).is_err() {
        warn_armed_at_not_persisted();
    }
    now
}

/// Drop the armed-at row when no schedule is configured, so re-enabling later
/// starts a fresh clock. Best-effort like the arming write.
fn disarm_schedule_clock(store: &ServerMetaStore) -> u64 {
    if store.remove_backup_armed_at_unix_secs().is_err() {
        warn_armed_at_not_persisted();
    }
    0
}

fn warn_armed_at_not_persisted() {
    tracing::warn!(
        reason = "persist-armed-at-failed",
        "backup schedule arming stamp was not persisted"
    );
}

/// Persist the stamp this process just published, best-effort.
///
/// Never fails the run: a missing handle (post-shutdown or a process that never
/// booted a server) is a no-op, and an I/O failure logs a bounded token only —
/// no path, no raw `Display` — so a broken data dir cannot leak into logs or
/// abort a completed copy.
fn persist_last_success(timestamp_seconds: u64) {
    let holder = META_STORE.load();
    let Some(store) = holder.as_deref().and_then(|weak| weak.upgrade()) else {
        return;
    };
    if store
        .store_backup_last_success_unix_ns(timestamp_seconds)
        .is_err()
    {
        tracing::warn!(
            reason = "persist-last-success-failed",
            "backup last-success stamp was not persisted"
        );
    }
}

/// Wall clock in epoch SECONDS, or the pinned test clock when one is set.
fn unix_seconds_now() -> u64 {
    #[cfg(test)]
    {
        let pinned = CLOCK_OVERRIDE_SECONDS.load(Ordering::Relaxed);
        if pinned != 0 {
            return pinned;
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// TEST-ONLY wall clock override in epoch SECONDS; `0` = use the real clock.
///
/// Process-global, like the state it drives: every test that pins it must be
/// `#[serial_test::serial]` and must restore it through [`ClockOverride`].
#[cfg(test)]
static CLOCK_OVERRIDE_SECONDS: AtomicU64 = AtomicU64::new(0);

/// TEST-ONLY pinned wall clock: [`Self::set`] pins epoch SECONDS, `Drop`
/// restores the value that was pinned before it (normally `0` = real clock).
#[cfg(test)]
pub(crate) struct ClockOverride {
    previous: u64,
}

#[cfg(test)]
impl ClockOverride {
    /// Pin the clock to `seconds` until this guard drops.
    pub(crate) fn set(seconds: u64) -> Self {
        Self {
            previous: CLOCK_OVERRIDE_SECONDS.swap(seconds, Ordering::SeqCst),
        }
    }

    /// Move the pinned clock to `seconds`.
    pub(crate) fn pin(&self, seconds: u64) {
        CLOCK_OVERRIDE_SECONDS.store(seconds, Ordering::SeqCst);
    }

    /// Move the pinned clock forward by `delta` seconds.
    pub(crate) fn advance(&self, delta: u64) {
        CLOCK_OVERRIDE_SECONDS.fetch_add(delta, Ordering::SeqCst);
    }
}

#[cfg(test)]
impl Drop for ClockOverride {
    fn drop(&mut self) {
        CLOCK_OVERRIDE_SECONDS.store(self.previous, Ordering::SeqCst);
    }
}

/// TEST-ONLY setter for the exported stamp, so a test that shares this
/// process can start from a known value and restore `0` (no evidence) after.
#[cfg(test)]
pub(crate) fn set_export_last_success_for_test(seconds: u64) {
    STATE
        .export_last_success_seconds
        .store(seconds, Ordering::Release);
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
///
/// Runs the moment the protocol returns `Ok` — i.e. before retention — so the
/// durable stamp recorded here survives a restart even when retention or the
/// next interval tick never happens.
pub(super) fn note_success(bytes: u64, duration: Duration) {
    let timestamp_seconds = unix_seconds_now();
    // `fetch_max`: an in-process success wins over an older durable row and
    // re-raises a cell a scrape previously clamped down to "now".
    STATE
        .export_last_success_seconds
        .fetch_max(timestamp_seconds, Ordering::AcqRel);
    let successful = SuccessfulBackup { duration, bytes };
    STATE.last_success.store(Some(Arc::new(successful)));
    persist_last_success(timestamp_seconds);
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
