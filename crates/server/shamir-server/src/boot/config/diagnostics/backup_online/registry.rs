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
//! check, and an untagged copy; payload/directory replacement under unchanged
//! manifest bytes is not detected, so the root is single-writer by contract.
//! A refused/failed capture records nothing. A process restart loses the
//! bounded 64-record list; outage copies whose records are lost remain
//! untouched by selection. A late cancellation after protocol success with a
//! healthy marker skips retention and leaves the completed copy untagged and
//! unrecorded; the operator must verify and manually move or retire it.
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
//! Publication ordering makes partial state impossible: metrics publish
//! with the record lock held, `running = false` LAST. Either way the
//! `last_error` directs the operator to `verify` before use.

#[cfg(test)]
use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use shamir_db::ShamirDb;
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::copy_engine::CopyOptions;
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupPermit, OnlineBackupError, SealRecopyCap,
};

use super::adapter::{EngineBackupPort, PortRunState};
use super::metrics as backup_metrics;
use super::progress_attribution::{final_per_repo_progress, final_per_repo_progress_bytes};
use super::schedule::destination::{scheduled_preflight, validate_destination};
use super::schedule::retention_pass::{
    bounded_manifest_bytes, manifest_capture, scheduled_retention_pass, MarkerProvisioning,
    PendingAdoptions,
};
#[cfg(test)]
use shamir_db::engine::repo::RepoInstance;

/// Production seal re-copy cap: 8 s monotonic budget per repo including
/// the seal barrier, NO byte cap (R2a-4 follow-up 4; module doc). The
/// budget boundary is the absolute `Instant` handed to
/// `OnlineBackupPort::seal_point_before`, so the barrier itself counts;
/// the copy engine checks it cooperatively INSIDE its I/O paths.
/// Wall-clock caps cannot preempt in-flight syscalls (fail-closed
/// overrun documented in the module doc).
const SEAL_RECOPY_MAX_SECONDS: u64 = 8;

/// The production cap (used by `RunJob` and the cap regression test):
/// 8 s per repo, `max_bytes = None` — the seal re-copy is a full-file
/// rewrite, so no size threshold belongs here (module doc).
pub(crate) fn production_seal_cap() -> SealRecopyCap {
    SealRecopyCap {
        max_bytes: None,
        max_seconds: Some(SEAL_RECOPY_MAX_SECONDS),
    }
}

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

/// Immutable snapshot of the running/last backup (design §4.7 `BackupStatus`
/// payload shape; the wire mapping is a later step).
#[derive(Debug, Clone)]
pub struct BackupStatusSnapshot {
    pub backup_id: String,
    /// The operator-supplied destination root.
    pub destination: PathBuf,
    /// The actual `<destination>/<timestamp>` directory, known on success
    /// (the protocol removes it on every failure path).
    pub dest_dir: Option<PathBuf>,
    pub started_at_unix_ns: u128,
    pub running: bool,
    pub cancel_requested: bool,
    /// Live stage: the adapter's port-call hints while running, the
    /// protocol's final stage once finished.
    pub stage: BackupStage,
    /// Progress counters; per-repo hashed bytes/files are updated by the
    /// protocol internally and become visible when the run finishes (the
    /// protocol exposes no progress callback — reported gap).
    pub progress: BackupProgress,
    /// Repos excluded from the captured set with reasons (Q8/Q9).
    pub excluded_repos: Vec<String>,
    /// Repos in the step-A snapshot.
    pub repos_in_snapshot: usize,
    /// Catalogue rows beyond the snapshot at run end (created during the
    /// backup, excluded per Q9).
    pub repos_created_during: usize,
    pub last_error: Option<String>,
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
    pending_step_callback: Mutex<Option<TestStepCallback>>,
    /// Test-only (R2d-FIX-C): a one-shot post-run callback the NEXT
    /// `start` moves onto its runner thread, invoked once after a
    /// SUCCESSFUL protocol return and before the guard stamp, the
    /// completion capture and the retention pass — fixture surgery (e.g.
    /// renaming the published child to a fixed name) with no production
    /// seam.
    #[cfg(test)]
    pending_post_run_callback: Mutex<Option<TestPostRunCallback>>,
}

/// Test-only step callback: invoked by the engine's barrier-step hook
/// with the step name, on the armed runner thread.
#[cfg(test)]
type TestStepCallback = Box<dyn Fn(&'static str) + Send>;

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
    /// The receiver reports protocol success after runner cleanup; a panic
    /// closes it without a value.
    pub(super) fn start_scheduled(
        &self,
        destination: PathBuf,
        owner: String,
        policy: BackupRetentionConfig,
    ) -> Result<oneshot::Receiver<bool>, StartBackupError> {
        let (completion, receiver) = oneshot::channel();
        self.start_inner(
            destination,
            Some(ScheduledRun { owner, policy }),
            Some(completion),
        )?;
        Ok(receiver)
    }

    fn start_inner(
        &self,
        to: PathBuf,
        scheduled: Option<ScheduledRun>,
        completion: Option<oneshot::Sender<bool>>,
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
        let guard = RunGuard::new(
            Arc::clone(&self.record),
            Arc::clone(&job.run_state),
            started_at_unix_ns,
            to.clone(),
            completion,
        );
        let spawned = std::thread::Builder::new()
            .name("online-backup".to_string())
            .spawn(move || job.run(guard));
        if let Err(spawn_error) = spawned {
            let stage = BackupStage::Preparing;
            let message = format!("online-backup runner thread failed to spawn: {spawn_error}");
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
    step_callback: Option<TestStepCallback>,
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
        // Test-only: arm this runner thread with the run's one-shot step
        // callback (the shared engine hook consults the per-thread slot;
        // an unarmed thread is a no-op).
        #[cfg(test)]
        if let Some(callback) = step_callback {
            install_test_step_hook();
            TEST_STEP_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
        }
        let started_at = Instant::now();
        let Some(permit_guard) = permit.try_acquire() else {
            // Defensive: the registry gate makes this unreachable today;
            // record a clean failure instead of racing the protocol.
            let mut record = record.lock();
            record.last_error = Some("another online backup holds the permit".to_string());
            backup_metrics::note_finished(true, BackupStage::Preparing, 0, started_at.elapsed());
            record.running = false;
            drop(record);
            guard.finish(false);
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
                    tracing::warn!(
                        root = %to.display(),
                        %error,
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
        // no writer admission. `record.running` stays `true` until the
        // publication below, so admission and cancel semantics are
        // unchanged; as a local dropped before the `guard` parameter, the
        // unwind order (permit, then failure publication) is preserved.
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
        // Scheduled retention reports marker-provisioning failures with the
        // same outcome and metric as failed pruning, independent of the copy.
        // Provenance is snapshotted and updated under the existing record
        // lock; filesystem capture happened before locking. Marker failure
        // observed before late cancellation still reaches the failure metric,
        // and successful capture records provenance. A late cancellation
        // after protocol success with a healthy marker skips prune, leaving
        // the copy untagged and unrecorded for operator verification/retirement.
        if let Some(scheduled) = scheduled {
            let recorded = record
                .lock()
                .pending_adoptions
                .recorded_for(&to, &scheduled.owner);
            let retention = scheduled_retention_pass(
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
                                // provenance; manifest equality is not proof
                                // of payload/directory identity, and refused
                                // capture skips fallback attribution reads.
                                tracing::warn!(
                                    dest_dir = %report.dest_dir.display(),
                                    "bounded manifest capture or decode was refused; no adoption \
                                     provenance was stored and the copy stays untouched by selection"
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
                        &retention.adopted,
                    );
                }
                MarkerProvisioning::NotAttempted => {}
            }
        }
        // Owner-side final progress: the protocol notes zeroed per-repo
        // entries and exposes no progress callback. Successful bounded
        // capture supplies attribution; a refused capture leaves zero/unknown
        // fields and deliberately skips the ordinary unbounded fallback read.
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
        let stage = outcome
            .as_ref()
            .err()
            .map_or(progress.stage, OnlineBackupError::stage);
        // Publish under the record lock, metrics first, `running = false`
        // LAST — a final record is atomic (the unwind guard relies on
        // this: it re-publishes nothing once `running` is false).
        let mut record = record.lock();
        record.progress = progress;
        record.repos_created_during = repos_created_during;
        record.dest_dir = outcome.as_ref().ok().map(|report| report.dest_dir.clone());
        record.last_error = outcome.as_ref().err().map(full_error_chain);
        backup_metrics::note_finished(failed, stage, bytes, started_at.elapsed());
        record.running = false;
        drop(record);
        guard.finish(!failed);
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
/// Publication ordering makes partial state impossible: the body
/// publishes metrics with the record lock held and sets `running = false`
/// LAST, so a final record is atomic — the guard re-publishes nothing
/// and re-advances no counter. Either way `last_error` directs the
/// operator to `verify` before use.
struct RunGuard {
    record: Arc<Mutex<RunRecord>>,
    /// The run's live stage hint, read at unwind time (before the permit
    /// drops, the adapter's port-call hints are still consistent).
    run_state: Arc<PortRunState>,
    started_at_unix_ns: u128,
    /// The OPERATOR destination root (the snapshot child is created by
    /// the protocol under it).
    destination: PathBuf,
    /// Set by the runner body IMMEDIATELY after the protocol returned
    /// `Ok` — the exact, atomically created `<to>/<timestamp>` snapshot
    /// directory the protocol verified and kept. No filesystem probing
    /// at unwind time: an unrelated `manifest.json` under the operator
    /// root must never be mistaken for a completed run.
    completed_dir: Option<PathBuf>,
    started_at: Instant,
    armed: bool,
    // Field drop closes the channel on unwind only after Drop publishes
    // the failure. Normal completion explicitly sends after publication.
    completion: Option<oneshot::Sender<bool>>,
}

impl RunGuard {
    fn new(
        record: Arc<Mutex<RunRecord>>,
        run_state: Arc<PortRunState>,
        started_at_unix_ns: u128,
        destination: PathBuf,
        completion: Option<oneshot::Sender<bool>>,
    ) -> Self {
        Self {
            record,
            run_state,
            started_at_unix_ns,
            destination,
            completed_dir: None,
            started_at: Instant::now(),
            armed: false,
            completion,
        }
    }

    fn finish(&mut self, verified: bool) {
        self.armed = false;
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(verified);
        }
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let stage = self.run_state.stage();
        let error = match &self.completed_dir {
            Some(dir) => format!(
                "online-backup runner panicked after the protocol completed; the self-verified \
                 snapshot under {} is KEPT — run `verify` before use",
                dir.display()
            ),
            None => format!(
                "online-backup runner panicked before publishing a result (stage hint {stage:?}); \
                 any files the protocol may have left under {} must be re-verified with `verify` \
                 before use",
                self.destination.display()
            ),
        };
        let mut record = self.record.lock();
        if record.started_at_unix_ns != self.started_at_unix_ns {
            // A replacement run (or the failed-spawn reset in `start`)
            // owns the record: nothing of THIS run may be published.
            return;
        }
        if !record.running {
            // The body fully published under the record lock (metrics
            // first, `running = false` LAST) before disarming: a final
            // record is atomic, so nothing is missing here and no
            // counter may be re-advanced.
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
        backup_metrics::note_finished(true, stage, 0, self.started_at.elapsed());
        record.running = false;
    }
}

// THE shared engine-step dispatcher for this test binary (R2a-4): the
// engine's `ONLINE_BACKUP_STEP_HOOK` slot is a first-wins
// `fn(&'static str)` `OnceLock`, so every test in this binary that wants
// barrier-step callbacks must go through this one function instead of
// installing its own hook (two competing installers would silently
// break the loser). Dispatch is per-THREAD: the hook is a no-op on any
// thread that has not armed a callback, so arming is per-run/per-thread
// state — never process-global.
#[cfg(test)]
thread_local! {
    static TEST_STEP_CALLBACK: RefCell<Option<TestStepCallback>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn shared_test_step_hook(step: &'static str) {
    TEST_STEP_CALLBACK.with(|slot| {
        if let Some(callback) = slot.borrow().as_ref() {
            callback(step);
        }
    });
}

/// Install the shared dispatcher into the engine. The engine's setter is
/// itself idempotent (first install wins), so this is safe to call from
/// every arming site.
#[cfg(test)]
pub(crate) fn install_test_step_hook() {
    RepoInstance::set_online_backup_step_hook(shared_test_step_hook);
}

/// Arm the CURRENT thread: `on_step` fires exactly once when the engine's
/// barrier reaches `step` on this thread (one-shot; later steps run
/// unimpeded). Callable from any test that drives its own backup/bridge
/// thread — arm inside the driving thread before the barrier runs. Never
/// process-global.
#[cfg(test)]
pub(crate) fn arm_backup_step(step: &'static str, on_step: impl FnOnce() + Send + 'static) {
    install_test_step_hook();
    let fired_once = std::cell::RefCell::new(Some(on_step));
    TEST_STEP_CALLBACK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |fired| {
            if fired == step {
                if let Some(on_step) = fired_once.borrow_mut().take() {
                    on_step();
                }
            }
        }));
    });
}

#[cfg(test)]
impl OnlineBackupRegistry {
    /// Arm a one-shot step callback for this registry's NEXT `start`: the
    /// callback is moved onto that run's runner thread (and only that
    /// thread) before the protocol runs, and fires at each engine
    /// barrier-step boundary through the shared dispatcher
    /// ([`install_test_step_hook`]). Per-registry opt-in: no other run,
    /// registry instance or test observes it.
    pub(crate) fn arm_runner_step_callback(&self, callback: TestStepCallback) {
        *self.pending_step_callback.lock() = Some(callback);
    }

    #[cfg(test)]
    pub(super) fn pending_adoption_count_for_test(&self, root: &Path, owner: &str) -> usize {
        self.record
            .lock()
            .pending_adoptions
            .recorded_for(root, owner)
            .len()
    }

    /// Arm a one-shot post-run callback for this registry's NEXT `start`:
    /// after the protocol returns `Ok` on the runner thread and before the
    /// guard stamp, completion capture and retention pass, it may rename the
    /// published child (`None` keeps the report path). Per-registry opt-in.
    pub(super) fn arm_post_run_callback(&self, callback: TestPostRunCallback) {
        *self.pending_post_run_callback.lock() = Some(callback);
    }
}

/// Best-effort live catalogue row count; a failed read reports 0 (the
/// created-during note then reads 0 rather than failing the record).
fn live_repo_count(db: &ShamirDb, runtime: &Handle) -> usize {
    let stores = db.system_store();
    runtime
        .block_on(Box::pin(stores.load_repositories()))
        .map(|records| records.len())
        .unwrap_or(0)
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

/// The error's `Display` chain, top error first. The protocol's top-level
/// `Display` already carries the stage (`failed at stage Sealing`); the
/// source chain contributes the typed failure detail (e.g. the
/// `RecopyCap` bytes-vs-cap text) that `to_string()` alone would drop,
/// so `BackupStatus`'s `last_error` keeps both stage and cap detail.
fn full_error_chain(error: &OnlineBackupError) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(link) = source {
        text.push_str(": ");
        text.push_str(&link.to_string());
        source = link.source();
    }
    text
}
