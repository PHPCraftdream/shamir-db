//! Online-backup session registry and background runner (R2a step 3a).
//!
//! One online backup per server (design §4.4 / D9): the in-process
//! `BackupPermit` guards the engine protocol; this registry additionally
//! gates `start` so the typed `AlreadyRunning` answer is synchronous. Work
//! runs on a named blocking thread (the port is synchronous); async engine
//! calls are bridged through the captured runtime handle — no async work is
//! started under a lock, nothing blocking runs on the runtime workers.
//! Status survives until the next `start` (design §4.7); `cancel` flips the
//! protocol's `CancelFlag`, whose cleanup removes the destination and
//! releases pins/seals (R4).
//!
//! Snapshot lease (R2d-FIX-C): the protocol's private self-check uses the
//! crate-private unleased entry (`verify_writer_snapshot`) only for the
//! fresh child it just created; it takes no lease and cannot unlease an
//! older sibling. There is NO thread-scoped bypass anymore. Scheduled runs
//! provision the lease marker BEFORE publishing the snapshot. A provisioning
//! failure is a retention failure with the same outcome and metric as a
//! failed prune, independent of copy outcome. A successful copy from such a
//! run is recorded as bounded IN-MEMORY provenance only if the post-protocol
//! bounded manifest capture succeeds and decodes (root + owner + name +
//! creation time + manifest-byte digest). The capture happens after protocol
//! return: it records the manifest bytes observed then, not proof of the
//! protocol's original payload tree or directory identity. Adoption requires
//! those manifest bytes/time to remain unchanged at capture, scan and late
//! check; an untagged copy is tagged, and a copy already bearing this owner's
//! tag is reported as adopted without a rewrite. Payload/directory
//! replacement under unchanged manifest bytes is not detected, so the root is
//! single-writer by contract.
//! A refused/failed capture records nothing. A process restart loses the
//! bounded 64-record list; outage copies whose records are lost remain
//! untouched by selection. A cancellation observed after protocol success and
//! before tagging starts, with a healthy marker, skips retention and leaves
//! the completed copy untagged and unrecorded; the operator must verify and
//! manually move or retire it. A cancellation after tagging began leaves the
//! fresh copy tagged and without an adoption record.
//!
//! Destination handling (§4.2 A): the operator `to` must be absolute and is
//! validated exactly like the offline backup — canonicalize the nearest
//! existing ancestor and refuse anything inside (or equal to) the source
//! data dir (offline precedent: `shamir-sync` `offline.rs::
//! reject_destination_inside_source`, private there, mirrored here); the
//! `<to>/<timestamp>/` directory is then created atomically by the protocol
//! itself, which refuses existing directories.
//!
//! Production seal re-copy cap (R2a-4 follow-up 4): an 8 s monotonic
//! budget per repo INCLUDING the seal barrier, and NO byte cap
//! (`max_bytes = None`). The byte cap was removed deliberately: the seal
//! re-copy is a full-file rewrite, not a delta — the bytes copied depend
//! on file sizes, not on how much changed, so an arbitrary threshold
//! failed legitimate large WALs while adding nothing to the real bound.
//! The binding limits are the two clocks: the 8 s per-repo cooperative
//! budget (checked INSIDE the copy engine's I/O paths — before every
//! chunk write, around every fsync, with the sharing-retry sleeps
//! clamped — never as a detached kill) and the engine's independent 15 s
//! leader-wait cap on the barrier itself, which stays the larger side of
//! the pair. A repo whose re-copy cannot finish inside 8 s fails closed
//! (typed error at `Sealing`, destination removed, commits resume);
//! explicitly configured tiny byte caps remain supported with their
//! typed `RecopyCap` failure.
//!
//! Panic safety (R2a-4 defect 4): a panic on the detached runner thread
//! must not leave `running` stuck at `true` or the §4.8 metrics at
//! `active = 1`. Every run carries a [`RunGuard`] created in `start` and
//! moved into the closure before spawn; on the unwind path (the guard is
//! a `run` parameter, so it drops AFTER the permit and after the
//! protocol's own pins/seals RAII deeper in `run_online_backup`) it
//! publishes a failed record — guarded by the run's `started_at_unix_ns`
//! so a replacement run's record is never clobbered — and closes the
//! metrics. The runner body stamps the guard with the exact verified
//! snapshot path the moment the protocol returns `Ok`, so the guard
//! distinguishes a COMPLETED, self-verified snapshot — recorded, never
//! deleted — from partial protocol output (removed by the protocol's
//! unwind cleanup; no removal is promised that may not have happened).
//! Publication ordering: the COPY outcome (`note_finished` plus the
//! scheduled outcome `oneshot`) publishes at protocol completion, BEFORE
//! retention, so a long retention never keeps the scheduler's failure
//! streak active and the completion survives a retention panic. A second,
//! LATE `oneshot` (FINISHED) resolves only when the run is fully over —
//! after `record.running = false`, on the defensive permit-refusal path,
//! or on the unwind path after the failed record — so shutdown can wait
//! for retention and the final publication without making admission wait.
//! Retention + adoption then run under `catch_unwind` (a panic there is
//! ONE retention failure, never a copy one); `record.running` stays `true`
//! until the final record publication sets it `false` LAST, and the guard
//! is disarmed only then. The same accounting holds for an unwind in the
//! window between publication and the end of retention: the guard reads its
//! `copy_accounted` flag, set when `publish_outcome` publishes the copy
//! (independently of taking the sender), so it records ONE retention
//! failure for a SCHEDULED run and none for a MANUAL one (no retention
//! pass); a copy outcome already published is never counted
//! twice. Only a panic BEFORE publication costs the guard's own copy
//! failure. `last_error` is path-free (`stage: reason`) for a SCHEDULED
//! run and the full `Display` chain for a MANUAL one; either way it
//! directs the operator to `verify` before use.

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use shamir_db::ShamirDb;
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::copy_engine::CopyOptions;
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, stage_and_io_reason, BackupPermit, OnlineBackupError,
};

use super::adapter::{EngineBackupPort, PortRunState};
use super::metrics as backup_metrics;
use super::progress_attribution::{
    final_per_repo_progress, final_per_repo_progress_bytes, full_error_chain, live_repo_count,
    unix_nanos,
};
use super::schedule::destination::{scheduled_preflight, validate_destination};
use super::schedule::retention_pass::{
    bounded_manifest_bytes, manifest_capture, scheduled_retention_pass, MarkerProvisioning,
    PendingAdoptions,
};

pub(crate) use super::progress_attribution::production_seal_cap;
pub use super::progress_attribution::BackupStatusSnapshot;

#[cfg(test)]
pub(crate) use super::schedule::online_backup_step_hooks::{
    arm_backup_step, install_test_step_hook, set_thread_step_callback, BackupStepCallback,
};
#[cfg(test)]
use super::schedule::retention_pass::fire_post_publication_panic;
#[cfg(test)]
pub(crate) use super::schedule::retention_pass::set_scheduled_retention_end as arm_scheduled_retention_end;
#[cfg(test)]
pub(crate) use super::schedule::retention_pass::set_scheduled_retention_pause as arm_scheduled_retention_pause;

/// Failure causes of a `start`.
#[derive(Debug, thiserror::Error)]
pub enum StartBackupError {
    /// One online backup is already running (design D9).
    #[error("another online backup is already running")]
    AlreadyRunning,
    /// The operator destination is invalid (relative, inside the data dir).
    #[error("invalid backup destination: {0}")]
    InvalidDestination(String),
    /// Destination setup or thread spawn failed on I/O.
    #[error("io error starting the backup: {0}")]
    Io(#[from] io::Error),
}

/// Mutable record of the running/last backup, shared between the registry
/// and the runner thread (the thread never borrows the registry).
struct RunRecord {
    running: bool,
    cancel: CancelFlag,
    run: Arc<PortRunState>,
    progress: BackupProgress,
    destination: PathBuf,
    dest_dir: Option<PathBuf>,
    started_at_unix_ns: u128,
    repos_created_during: usize,
    last_error: Option<String>,
    /// Bounded in-memory adoption provenance (module doc). Carried across
    /// replacement runs via `start_inner`; lost on process restart by
    /// design (documented limitation).
    pending_adoptions: PendingAdoptions,
}

/// Registry over the shared engine handle. Chunk 4 wires the admin ops to
/// `start` / `status` / `cancel`.
pub struct OnlineBackupRegistry {
    /// Weak: the engine owns this registry (through the installed admin
    /// port), so a strong handle would be a reference cycle that keeps every
    /// store open after the server shuts down. A run upgrades it for its
    /// whole duration only.
    db: Weak<ShamirDb>,
    data_dir: PathBuf,
    runtime: Handle,
    permit: Arc<BackupPermit>,
    record: Arc<Mutex<RunRecord>>,
    backup_id: Mutex<Option<String>>,
    /// The server's root shutdown token, when running under the real
    /// launcher: each run's `CancelFlag` is then backed by a CHILD token
    /// (`CancelFlag::from_shutdown` derives it per run), so server shutdown
    /// cancels the run while an admin `CancelBackup` cancels only the child
    /// (never the server). `None` keeps the default flag: a fresh,
    /// never-cancelled token (the constructor existing tests use).
    shutdown: Option<CancellationToken>,
    /// Test-only (R2a-4 defect 4): a one-shot step callback the NEXT
    /// `start` moves onto its runner thread, which installs it as this
    /// thread's arm of the SHARED engine-step dispatcher
    /// ([`install_test_step_hook`]) before the protocol runs — per-run,
    /// per-thread state; no process-global arming.
    #[cfg(test)]
    pending_step_callback: Mutex<Option<BackupStepCallback>>,
    /// Test-only (R2d-FIX-C): a one-shot post-run callback the NEXT
    /// `start` moves onto its runner thread, invoked once after a
    /// SUCCESSFUL protocol return and before the guard stamp, the
    /// completion capture and the retention pass — fixture surgery (e.g.
    /// renaming the published child to a fixed name) with no production
    /// seam.
    #[cfg(test)]
    pending_post_run_callback: Mutex<Option<TestPostRunCallback>>,
}

/// Test-only post-run callback: given the protocol's published snapshot
/// directory, returns its (possibly renamed) location — `None` leaves the
/// report untouched.
#[cfg(test)]
type TestPostRunCallback = Box<dyn Fn(&Path) -> Option<PathBuf> + Send>;

impl OnlineBackupRegistry {
    /// Build the registry. `runtime` must be the server's runtime handle
    /// (the runner bridges async facade calls through it from a blocking
    /// thread). `data_dir` is the server's source data dir. Runs cancel
    /// through the default flag — a fresh, never-cancelled token (the
    /// constructor existing tests use); the production launcher uses
    /// [`OnlineBackupRegistry::new_with_shutdown`].
    pub fn new(db: Arc<ShamirDb>, data_dir: PathBuf, runtime: Handle) -> Self {
        Self::new_with_shutdown_inner(db, data_dir, runtime, None)
    }

    /// Build the registry wired to the server's root shutdown token: every
    /// run started here takes a child token, so server shutdown cancels the
    /// in-flight run while an admin `CancelBackup` cancels only that run's
    /// child (never the server itself).
    pub fn new_with_shutdown(
        db: Arc<ShamirDb>,
        data_dir: PathBuf,
        runtime: Handle,
        shutdown: CancellationToken,
    ) -> Self {
        Self::new_with_shutdown_inner(db, data_dir, runtime, Some(shutdown))
    }

    fn new_with_shutdown_inner(
        db: Arc<ShamirDb>,
        data_dir: PathBuf,
        runtime: Handle,
        shutdown: Option<CancellationToken>,
    ) -> Self {
        let fresh = Self::fresh_record(&data_dir);
        Self {
            db: Arc::downgrade(&db),
            data_dir,
            runtime,
            permit: Arc::new(BackupPermit::new()),
            record: Arc::new(Mutex::new(fresh)),
            backup_id: Mutex::new(None),
            shutdown,
            #[cfg(test)]
            pending_step_callback: Mutex::new(None),
            #[cfg(test)]
            pending_post_run_callback: Mutex::new(None),
        }
    }

    fn fresh_record(data_dir: &Path) -> RunRecord {
        RunRecord {
            running: false,
            cancel: CancelFlag::default(),
            run: Arc::new(PortRunState::new()),
            progress: BackupProgress::new(),
            destination: data_dir.to_path_buf(),
            dest_dir: None,
            started_at_unix_ns: 0,
            repos_created_during: 0,
            last_error: None,
            pending_adoptions: PendingAdoptions::default(),
        }
    }

    /// Validate, register and start a backup; returns immediately with the
    /// backup id and destination root. The protocol's atomic
    /// `<to>/<timestamp>/` create happens on the runner thread (§4.2 A).
    pub fn start(&self, to: PathBuf) -> Result<(String, PathBuf), StartBackupError> {
        self.start_inner(to, None, None)
    }

    /// Admit a scheduled run without filesystem work on the scheduler task.
    pub(super) fn start_scheduled(
        &self,
        destination: PathBuf,
        owner: String,
        policy: BackupRetentionConfig,
    ) -> Result<ScheduledRunSignals, StartBackupError> {
        let (completion, outcome) = oneshot::channel();
        let (finished, finished_receiver) = oneshot::channel();
        self.start_inner(
            destination,
            Some(ScheduledRun { owner, policy }),
            Some(RunSignals {
                outcome: completion,
                finished,
            }),
        )?;
        Ok(ScheduledRunSignals {
            outcome,
            finished: finished_receiver,
        })
    }

    fn start_inner(
        &self,
        to: PathBuf,
        scheduled: Option<ScheduledRun>,
        signals: Option<RunSignals>,
    ) -> Result<(String, PathBuf), StartBackupError> {
        if scheduled.is_none() {
            validate_destination(&to, &self.data_dir)?;
        }
        let db = self
            .db
            .upgrade()
            .ok_or_else(|| StartBackupError::Io(io::Error::other("the engine is shutting down")))?;
        // The protocol creates only the timestamped child (atomic create,
        // refusing existing dirs); the root must exist by then.
        if scheduled.is_none() {
            std::fs::create_dir_all(&to)?;
        }
        let started_at_unix_ns = unix_nanos();
        let backup_id = format!("ob-{started_at_unix_ns}");
        // Under the real launcher the run cancels on a CHILD token
        // `from_shutdown` derives from the server's root shutdown token:
        // shutdown cancels the run, an admin `CancelBackup` (below,
        // `record.cancel.cancel()`) cancels only the child. Cheap and
        // synchronous.
        let cancel = match &self.shutdown {
            Some(root) => CancelFlag::from_shutdown(root),
            None => CancelFlag::default(),
        };
        {
            let mut record = self.record.lock();
            if scheduled.is_some() && cancel.is_cancelled() {
                return Err(StartBackupError::Io(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "the server is shutting down",
                )));
            }
            if record.running {
                return Err(StartBackupError::AlreadyRunning);
            }
            // Provenance outlives the single-run record slot.
            let pending_adoptions = std::mem::take(&mut record.pending_adoptions);
            *record = RunRecord {
                running: true,
                cancel: cancel.clone(),
                run: Arc::new(PortRunState::new()),
                progress: BackupProgress::new(),
                destination: to.clone(),
                dest_dir: None,
                started_at_unix_ns,
                repos_created_during: 0,
                last_error: None,
                pending_adoptions,
            };
            *self.backup_id.lock() = Some(backup_id.clone());
        };
        // Test-only: take a pending per-run step callback (see
        // `arm_runner_step_callback`) onto this run's job, along with the
        // one-shot post-run fixture callback (see `arm_post_run_callback`).
        #[cfg(test)]
        let step_callback = self.pending_step_callback.lock().take();
        #[cfg(test)]
        let post_run_callback = self.pending_post_run_callback.lock().take();
        let scheduled_run = scheduled.is_some();
        let job = RunJob {
            db,
            data_dir: self.data_dir.clone(),
            runtime: self.runtime.clone(),
            permit: Arc::clone(&self.permit),
            cancel,
            to: to.clone(),
            scheduled,
            run_state: self.record.lock().run.clone(),
            record: Arc::clone(&self.record),
            #[cfg(test)]
            step_callback,
            #[cfg(test)]
            post_run_callback,
        };
        // §4.8 metrics hook: the run counts as started once registration
        // succeeded; a failed spawn is published as a preparing-stage
        // failure so `shamir_backup_active` cannot stick at 1 and the
        // registry is left idle. The `started_at_unix_ns` check makes the
        // publish safe against a replacement run (impossible today — the
        // stuck `running` flag refuses concurrent starts — but the guard
        // contract is uniform).
        backup_metrics::note_started();
        // The guard is created BEFORE the spawn and moved into the closure;
        // it arms itself only once the runner body begins, so a failed
        // spawn drops it inertly and the failure is published here instead.
        let (outcome, finished) = match signals {
            Some(signals) => (Some(signals.outcome), Some(signals.finished)),
            None => (None, None),
        };
        let guard = RunGuard::new(
            Arc::clone(&self.record),
            Arc::clone(&job.run_state),
            started_at_unix_ns,
            to.clone(),
            scheduled_run,
            outcome,
            finished,
        );
        let spawned = std::thread::Builder::new()
            .name("online-backup".to_string())
            .spawn(move || job.run(guard));
        if let Err(spawn_error) = spawned {
            let stage = BackupStage::Preparing;
            // A SCHEDULED run stores the path-free vocabulary; a manual run
            // keeps the raw OS text in its `Display` chain.
            let message = if scheduled_run {
                stage_and_io_reason(stage, spawn_error.kind())
            } else {
                format!("online-backup runner thread failed to spawn: {spawn_error}")
            };
            let mut record = self.record.lock();
            if record.running && record.started_at_unix_ns == started_at_unix_ns {
                record.progress.record_failure(stage, &message);
                record.dest_dir = None;
                record.last_error = Some(message);
                backup_metrics::note_finished(true, stage, 0, Duration::ZERO);
                record.running = false;
            }
            return Err(StartBackupError::Io(io::Error::other(
                spawn_error.to_string(),
            )));
        }
        Ok((backup_id, to))
    }

    /// Snapshot of the running/last backup.
    pub fn status(&self) -> BackupStatusSnapshot {
        let record = self.record.lock();
        let stage = if record.running {
            record.run.stage()
        } else {
            record.progress.stage
        };
        BackupStatusSnapshot {
            backup_id: self.backup_id.lock().clone().unwrap_or_default(),
            destination: record.destination.clone(),
            dest_dir: record.dest_dir.clone(),
            started_at_unix_ns: record.started_at_unix_ns,
            running: record.running,
            cancel_requested: record.cancel.is_cancelled(),
            stage,
            progress: record.progress.clone(),
            excluded_repos: record.run.excluded_repos(),
            repos_in_snapshot: record.run.snapshot_count(),
            repos_created_during: record.repos_created_during,
            last_error: record.last_error.clone(),
        }
    }

    /// Flip the cancellation flag; `true` if a run was active. Cleanup
    /// (destination removal, pins/seals released) is the protocol's job.
    pub fn cancel(&self) -> bool {
        let record = self.record.lock();
        if record.running {
            record.cancel.cancel();
            true
        } else {
            false
        }
    }
}

struct ScheduledRun {
    owner: String,
    policy: BackupRetentionConfig,
}

/// Both per-run channels of a scheduled run. `outcome` reports the COPY
/// outcome at protocol completion (before retention); `finished` resolves only
/// once the run is fully over (final record publication, defensive refusal, or
/// the unwind path after the failed record). A runner panic closes `outcome`
/// without a value and resolves `finished`, so the drain never hangs.
pub(super) struct ScheduledRunSignals {
    pub(super) outcome: oneshot::Receiver<bool>,
    pub(super) finished: oneshot::Receiver<()>,
}

/// The sender halves [`ScheduledRunSignals`] hands to the run's guard.
struct RunSignals {
    outcome: oneshot::Sender<bool>,
    finished: oneshot::Sender<()>,
}

/// Everything the runner thread owns for one backup.
struct RunJob {
    db: Arc<ShamirDb>,
    data_dir: PathBuf,
    runtime: Handle,
    to: PathBuf,
    scheduled: Option<ScheduledRun>,
    permit: Arc<BackupPermit>,
    cancel: CancelFlag,
    run_state: Arc<PortRunState>,
    record: Arc<Mutex<RunRecord>>,
    /// Test-only: armed on the runner thread before the protocol runs.
    #[cfg(test)]
    step_callback: Option<BackupStepCallback>,
    /// Test-only: invoked after a successful protocol return, before the
    /// guard stamp and the completion capture.
    #[cfg(test)]
    post_run_callback: Option<TestPostRunCallback>,
}

impl RunJob {
    /// The blocking-thread body: permit, protocol, record update. Never
    /// holds the record lock across the protocol call. `guard` is a
    /// parameter on purpose: parameters drop after the body's locals, so
    /// on an unwind the permit (and the protocol's own pins/seals RAII,
    /// deeper still) is released BEFORE the guard publishes the failure.
    fn run(self, mut guard: RunGuard) {
        guard.armed = true;
        let Self {
            db,
            data_dir,
            runtime,
            to,
            scheduled,
            permit,
            cancel,
            run_state,
            record,
            #[cfg(test)]
            step_callback,
            #[cfg(test)]
            post_run_callback,
        } = self;
        let scheduled_run = scheduled.is_some();
        // Test-only: arm this runner thread with the run's one-shot step
        // callback (the shared engine hook consults the per-thread slot;
        // an unarmed thread is a no-op).
        #[cfg(test)]
        if let Some(callback) = step_callback {
            install_test_step_hook();
            set_thread_step_callback(callback);
        }
        let started_at = Instant::now();
        let Some(permit_guard) = permit.try_acquire() else {
            // Defensive: the registry gate makes this unreachable today;
            // record a clean failure instead of racing the protocol. A
            // SCHEDULED run stores the path-free `preparing: busy`.
            let mut record = record.lock();
            record.last_error = Some(if scheduled_run {
                OnlineBackupError::Busy.stage_and_reason()
            } else {
                "another online backup holds the permit".to_string()
            });
            backup_metrics::note_finished(true, BackupStage::Preparing, 0, started_at.elapsed());
            record.running = false;
            drop(record);
            guard.publish_outcome(false);
            guard.disarm();
            return;
        };
        let port = EngineBackupPort::new(
            Arc::clone(&db),
            data_dir.clone(),
            runtime.clone(),
            run_state.clone(),
        );
        let mut progress = BackupProgress::new();
        let options = CopyOptions::default();
        let cap = production_seal_cap();
        let preflight = if scheduled.is_some() {
            scheduled_preflight(&to, &data_dir, &cancel)
        } else {
            Ok(())
        };
        let preflight_failed = preflight.is_err();
        // Publication-window guard (R2d-FIX-C): provision the marker BEFORE
        // A later pass may adopt the markerless copy only if bounded capture
        // succeeds and the manifest still matches; this records no payload or
        // directory identity. Refused capture leaves no provenance.
        let marker_provisioning = if preflight_failed || scheduled.is_none() {
            MarkerProvisioning::NotAttempted
        } else {
            match crate::sync::durability::backup::provision_snapshot_lease(&to) {
                Ok(()) => MarkerProvisioning::Provisioned,
                Err(error) => {
                    // Scheduled-only site: bounded `reason`/`kind`, no path.
                    tracing::warn!(
                        reason = "marker-unavailable",
                        kind = ?error.kind(),
                        "snapshot lease marker unavailable; retention fails for this run \
                         until the marker is repaired"
                    );
                    MarkerProvisioning::Failed
                }
            }
        };
        let outcome = preflight
            .inspect_err(|error| progress.record_failure(error.stage(), error))
            .and_then(|()| {
                // The protocol self-check uses its crate-private unleased
                // entry only for the exact fresh child it created. Ordinary
                // verification of a managed path still takes the shared lease.
                run_online_backup(
                    &port,
                    &data_dir,
                    &to,
                    &cancel,
                    &mut progress,
                    &options,
                    &cap,
                )
            });
        let copy_duration = started_at.elapsed();
        // Stamp the guard with the verified snapshot BEFORE any
        // panic-prone post-processing: from this point a runner panic
        // must leave the completed snapshot recorded, never deleted.
        // Release the BackupPermit BEFORE retention (R2d-FIX-C): prune needs
        // no writer admission. `record.running` stays `true` until the FINAL
        // publication after retention; as a local dropped before the `guard`
        // parameter, the unwind order (permit, then publication) holds.
        drop(permit_guard);
        // Test-only fixture surgery: rename the published child (fixed
        // name, fixed manifest creation time) BEFORE the guard stamps it,
        // so the guarded snapshot and the completion capture below see the
        // final output location and bytes. `Option::zip` keeps this
        // edition-2021-compatible without nested collapsible `if let`s.
        #[cfg(test)]
        let mut outcome = outcome;
        #[cfg(test)]
        if let Some((report, callback)) = outcome.as_mut().ok().zip(post_run_callback.as_ref()) {
            if let Some(renamed) = callback(&report.dest_dir) {
                report.dest_dir = renamed;
            }
        }
        if let Ok(report) = &outcome {
            guard.completed_dir = Some(report.dest_dir.clone());
            backup_metrics::note_success(report.total_bytes, copy_duration);
        }
        // Capture is attempted only for a completed run with failed marker
        // provisioning. The outer option distinguishes not attempted from an
        // attempted read; the inner result distinguishes success from refusal.
        let capture_outcome = match (&outcome, &marker_provisioning) {
            (Ok(report), MarkerProvisioning::Failed) => {
                Some(bounded_manifest_bytes(&report.dest_dir).ok_or(()))
            }
            _ => None,
        };
        let captured = capture_outcome
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|raw| manifest_capture(raw));
        // Owner-side final progress: the protocol notes zeroed per-repo
        // entries and exposes no progress callback. Successful bounded
        // capture supplies attribution; a refused capture leaves zero/unknown
        // fields and deliberately skips the ordinary unbounded fallback read.
        // Independent of retention: resolved BEFORE the outcome publishes.
        match &capture_outcome {
            Some(Ok(raw)) => {
                for (repo, bytes_hashed, files_hashed) in final_per_repo_progress_bytes(raw) {
                    progress.note_repo_progress(&repo, bytes_hashed, files_hashed);
                }
            }
            Some(Err(())) => {}
            None => {
                if let Ok(report) = &outcome {
                    for (repo, bytes_hashed, files_hashed) in final_per_repo_progress(report) {
                        progress.note_repo_progress(&repo, bytes_hashed, files_hashed);
                    }
                }
            }
        }
        let snapshot_count = run_state.snapshot_count();
        // Skip the post-run live catalogue read after a preflight failure or
        // cancellation. There is nothing to attribute, and `block_on` a
        // straggler store load only extends teardown.
        let repos_created_during = if cancel.is_cancelled() || preflight_failed {
            0
        } else {
            live_repo_count(&db, &runtime).saturating_sub(snapshot_count)
        };
        drop(port);
        drop(db);
        // §4.8 metrics hook values, prepared BEFORE the record lock: run
        // counters, copied bytes (the protocol's hashed-byte counters)
        // and the last run duration; the failure stage is where the
        // protocol recorded the failure.
        let bytes: u64 = progress.per_repo.iter().map(|repo| repo.bytes_hashed).sum();
        let failed = outcome.is_err();
        let failure = outcome.as_ref().err();
        let stage = failure.map_or(progress.stage, OnlineBackupError::stage);
        // Publish the COPY outcome here, BEFORE retention: the failure streak
        // stops tracking the copy as soon as it is known, and the completion
        // is delivered even if retention panics below (the guard stays ARMED).
        backup_metrics::note_finished(failed, stage, bytes, started_at.elapsed());
        guard.publish_outcome(!failed);
        // Test-only: a planted panic HERE proves what a panic in the
        // published-but-not-yet-retained window accounts for — ONE retention
        // failure, never a second copy failure.
        #[cfg(test)]
        fire_post_publication_panic();
        // Scheduled retention reports marker-provisioning failures with the
        // same outcome and metric as failed pruning, independent of the copy.
        // Provenance is snapshotted and updated under the existing record
        // lock; filesystem capture happened before locking. Marker failure
        // observed before late cancellation still reaches the failure metric,
        // and successful capture records provenance. A cancellation observed
        // before tagging starts, with a healthy marker, skips prune, leaving
        // the copy untagged and unrecorded for operator verification/retirement.
        //
        // A panic in here is a RETENTION failure — ONE increment of the
        // existing retention counter — never a copy failure, and it must not
        // prevent the final record publication below. `AssertUnwindSafe` is
        // sound: `RunRecord` is a `parking_lot::Mutex` (no poisoning; the lock
        // is released on unwind) and every mutation is one bounded move.
        if let Some(scheduled) = scheduled {
            let retention = catch_unwind(AssertUnwindSafe(|| {
                let recorded = record
                    .lock()
                    .pending_adoptions
                    .recorded_for(&to, &scheduled.owner);
                let pass = scheduled_retention_pass(
                    &to,
                    &scheduled.owner,
                    &scheduled.policy,
                    &cancel,
                    &outcome,
                    &marker_provisioning,
                    &recorded,
                );
                match marker_provisioning {
                    MarkerProvisioning::Failed => {
                        if let Ok(report) = &outcome {
                            match (
                                captured,
                                report.dest_dir.file_name().and_then(|n| n.to_str()),
                            ) {
                                (Some((created_at_unix_ns, manifest_digest)), Some(name)) => {
                                    record.lock().pending_adoptions.record(
                                        &to,
                                        &scheduled.owner,
                                        name,
                                        created_at_unix_ns,
                                        manifest_digest,
                                    );
                                }
                                (None, _) => {
                                    // Capture or decode refusal stores no
                                    // provenance; manifest equality is not
                                    // proof of payload/directory identity, and
                                    // refused capture skips fallback
                                    // attribution reads. Scheduled-only site:
                                    // `reason` replaces the path.
                                    tracing::warn!(
                                        reason = "manifest-capture-refused",
                                        "bounded manifest capture or decode was refused; no \
                                         adoption provenance was stored and the copy stays \
                                         untouched by selection"
                                    );
                                }
                                (Some(_), None) => tracing::warn!(
                                    "fresh snapshot name was not valid Unicode; retention cannot \
                                     adopt it"
                                ),
                            }
                        }
                    }
                    MarkerProvisioning::Provisioned => {
                        record.lock().pending_adoptions.retire_adopted(
                            &to,
                            &scheduled.owner,
                            &pass.adopted,
                        );
                    }
                    MarkerProvisioning::NotAttempted => {}
                }
            }));
            if retention.is_err() {
                backup_metrics::note_retention(0, 1, None);
                tracing::error!(
                    "scheduled retention panicked; accounted as one retention failure and the \
                     published copy outcome is unchanged"
                );
            }
        }
        // Final record publication, `running = false` LAST (the record stayed
        // `running` — admission refused — through retention).
        let mut record = record.lock();
        record.progress = progress;
        record.repos_created_during = repos_created_during;
        record.dest_dir = outcome.as_ref().ok().map(|report| report.dest_dir.clone());
        record.last_error = match (&outcome, scheduled_run) {
            (Err(error), true) => Some(error.stage_and_reason()),
            (Err(error), false) => Some(full_error_chain(error)),
            (Ok(_), _) => None,
        };
        // The scheduled run's LAST-RUN record publishes at the final
        // publication, before `running = false`: the run's own typed outcome
        // (completed / typed-cancelled / failed) with the stable path-free
        // vocabulary; a failure keeps the ONCE log with identical fields.
        if scheduled_run {
            backup_metrics::note_scheduled_run_outcome(&outcome, record.started_at_unix_ns);
        }
        record.running = false;
        drop(record);
        guard.disarm();
    }
}

/// Unwind guard of one registered run (R2a-4 defect 4): a panic on the
/// detached runner thread must not leave the record `running` or the §4.8
/// metrics with `shamir_backup_active` stuck at 1. Created in `start`
/// (disarmed — the runner body arms it, so a failed spawn drops it
/// inertly), moved into the spawn closure, and on the unwind path it
/// publishes a failed record and closes the metrics. The body sets the
/// guard's `completed_dir` the moment the protocol returns `Ok` (the
/// exact verified snapshot path), so the guard at unwind distinguishes a
/// COMPLETED, self-verified snapshot — recorded, never deleted — from a
/// partial output (already removed by the protocol's unwind cleanup;
/// the message never promises a removal that may have failed).
/// Publication ordering makes partial state impossible: the body publishes
/// the copy outcome at protocol completion, stays armed across retention,
/// then publishes the final record with `running = false` LAST and disarms —
/// the guard re-publishes nothing. A panic in the published window (the
/// `outcome` sender is already taken) accounts ONE retention failure for a
/// scheduled run, never a second copy counter.
struct RunGuard {
    record: Arc<Mutex<RunRecord>>,
    /// The run's live stage hint, read at unwind time (before the permit
    /// drops, the adapter's port-call hints are still consistent).
    run_state: Arc<PortRunState>,
    started_at_unix_ns: u128,
    /// The OPERATOR destination root (the snapshot child is created by
    /// the protocol under it).
    destination: PathBuf,
    /// SCHEDULED run: its unwind message is rendered path-free.
    scheduled: bool,
    /// Set by the runner body IMMEDIATELY after the protocol returned
    /// `Ok` — the exact, atomically created `<to>/<timestamp>` snapshot
    /// directory the protocol verified and kept. No filesystem probing
    /// at unwind time: an unrelated `manifest.json` under the operator
    /// root must never be mistaken for a completed run.
    completed_dir: Option<PathBuf>,
    started_at: Instant,
    armed: bool,
    /// Whether the body already accounted the COPY outcome (`note_finished`):
    /// an explicit flag, because a MANUAL run has no sender at all and
    /// `outcome.is_none()` could not tell "published" from "never published".
    copy_accounted: bool,
    // Field drop closes the channel on unwind only after Drop publishes
    // the failure; normal completion sends explicitly, before retention.
    outcome: Option<oneshot::Sender<bool>>,
    // The LATE signal: sent exactly once when the run is fully over (final
    // record published, defensive refusal, or the unwind path). Never early.
    finished: Option<oneshot::Sender<()>>,
}

impl RunGuard {
    fn new(
        record: Arc<Mutex<RunRecord>>,
        run_state: Arc<PortRunState>,
        started_at_unix_ns: u128,
        destination: PathBuf,
        scheduled: bool,
        outcome: Option<oneshot::Sender<bool>>,
        finished: Option<oneshot::Sender<()>>,
    ) -> Self {
        Self {
            record,
            run_state,
            started_at_unix_ns,
            destination,
            scheduled,
            completed_dir: None,
            started_at: Instant::now(),
            armed: false,
            copy_accounted: false,
            outcome,
            finished,
        }
    }

    /// Publish the COPY outcome at protocol completion; does NOT disarm (the
    /// guard must stay live across retention so a later panic still records).
    fn publish_outcome(&mut self, verified: bool) {
        // The accounting is done for EVERY run here; only a scheduled one has a sender.
        self.copy_accounted = true;
        if let Some(outcome) = self.outcome.take() {
            let _ = outcome.send(verified);
        }
    }

    /// Send the late FINISHED signal exactly once.
    fn send_finished(&mut self) {
        if let Some(finished) = self.finished.take() {
            let _ = finished.send(());
        }
    }

    /// Disarm after the final record publication (`running = false`) and send
    /// the late FINISHED signal; the run is fully over.
    fn disarm(&mut self) {
        self.armed = false;
        self.send_finished();
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let stage = self.run_state.stage();
        // `publish_outcome` marks the accounting done, so this covers the
        // scheduled post-publication window AND a manual run (which has no
        // sender) — the latter must still be counted as ONE copy failure.
        let copy_published = self.copy_accounted;
        let error = match (&self.completed_dir, self.scheduled) {
            (Some(_), true) => String::from(
                "online-backup runner panicked after the protocol completed; \
                 the self-verified snapshot was KEPT — run `verify` before use",
            ),
            (None, true) => format!(
                "online-backup runner panicked before publishing a result (stage hint {stage:?}); \
                 any files the protocol may have left must be re-verified with `verify` before use"
            ),
            (Some(dir), false) => format!(
                "online-backup runner panicked after the protocol completed; the self-verified \
                 snapshot under {} is KEPT — run `verify` before use",
                dir.display()
            ),
            (None, false) => format!(
                "online-backup runner panicked before publishing a result (stage hint {stage:?}); \
                 any files the protocol may have left under {} must be re-verified with `verify` \
                 before use",
                self.destination.display()
            ),
        };
        let mut record = self.record.lock();
        if record.started_at_unix_ns != self.started_at_unix_ns {
            // A replacement run (or the failed-spawn reset in `start`)
            // owns the record: nothing of THIS run may be published. The
            // late signal still resolves — this run IS over.
            drop(record);
            self.send_finished();
            return;
        }
        if !record.running {
            // The body fully published under the record lock (metrics
            // first, `running = false` LAST) before disarming: a final
            // record is atomic, so nothing is missing here and no
            // counter may be re-advanced.
            drop(record);
            self.send_finished();
            return;
        }
        // The guard finalizes the record: failed stage, the verified
        // snapshot's exact path recorded and never deleted (a partial
        // output was already removed by the protocol's unwind cleanup —
        // if that removal failed the message still only directs to
        // `verify`, it does not promise a removal that did not happen).
        record.progress.record_failure(stage, &error);
        record.dest_dir = self.completed_dir.clone();
        record.last_error = Some(error);
        if copy_published {
            // The copy outcome was already published at protocol completion:
            // a panic here is ONE retention failure for a SCHEDULED run (what
            // the module doc promises) and no counter at all for a MANUAL one,
            // which has no retention pass. The copy counters, `active`
            // included, are never touched twice.
            if self.scheduled {
                backup_metrics::note_retention(0, 1, None);
            }
        } else {
            backup_metrics::note_finished(true, stage, 0, self.started_at.elapsed());
        }
        record.running = false;
        drop(record);
        self.send_finished();
    }
}

// The engine-step dispatcher and arming helpers live in their OWN
// `schedule` module (`schedule::online_backup_step_hooks`, declared by
// `#[path]` over `schedule/tests/online_backup_step_hooks.rs`) and are
// re-exported above, keeping every call path. The arming seam impl touches
// private fields, so it lives in its OWN `#[cfg(test)]` module too (a NEW
// path over a file in `schedule/tests/`, intentionally NOT declared by
// `schedule/tests/mod.rs`): it is a child of THIS module, not of
// `schedule::tests`, and every call path is unchanged.

#[cfg(test)]
#[path = "schedule/tests/registry_seams.rs"]
mod registry_seams;
