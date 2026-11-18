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
//! Destination handling (§4.2 A): the operator `to` must be absolute and is
//! validated exactly like the offline backup — canonicalize the nearest
//! existing ancestor and refuse anything inside (or equal to) the source
//! data dir (offline precedent: `shamir-sync` `offline.rs::
//! reject_destination_inside_source`, private there, mirrored here); the
//! `<to>/<timestamp>/` directory is then created atomically by the protocol
//! itself, which refuses existing directories.
//!
//! Production seal re-copy cap (Q12): 4 GiB per repo and 1800 s wall clock —
//! fail-closed bounds on the writer-visibility pause; unlimited (the
//! `SealRecopyCap::default()`) would let a large store stall commits
//! unboundedly.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use shamir_db::ShamirDb;
use tokio::runtime::Handle;

use crate::sync::durability::backup::online::copy_engine::CopyOptions;
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupPermit, BackupReport, SealRecopyCap,
};

use super::adapter::{EngineBackupPort, PortRunState};
use super::metrics as backup_metrics;

/// Production seal re-copy cap: 4 GiB per repo, 1800 s wall clock (Q12;
/// documented in the step report — unlimited defaults are for tests).
const SEAL_RECOPY_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SEAL_RECOPY_MAX_SECONDS: u64 = 1800;

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
}

impl OnlineBackupRegistry {
    /// Build the registry. `runtime` must be the server's runtime handle
    /// (the runner bridges async facade calls through it from a blocking
    /// thread). `data_dir` is the server's source data dir.
    pub fn new(db: Arc<ShamirDb>, data_dir: PathBuf, runtime: Handle) -> Self {
        let fresh = Self::fresh_record(&data_dir);
        Self {
            db: Arc::downgrade(&db),
            data_dir,
            runtime,
            permit: Arc::new(BackupPermit::new()),
            record: Arc::new(Mutex::new(fresh)),
            backup_id: Mutex::new(None),
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
        }
    }

    /// Validate, register and start a backup; returns immediately with the
    /// backup id and destination root. The protocol's atomic
    /// `<to>/<timestamp>/` create happens on the runner thread (§4.2 A).
    pub fn start(&self, to: PathBuf) -> Result<(String, PathBuf), StartBackupError> {
        validate_destination(&to, &self.data_dir)?;
        let db = self
            .db
            .upgrade()
            .ok_or_else(|| StartBackupError::Io(io::Error::other("the engine is shutting down")))?;
        // The protocol creates only the timestamped child (atomic create,
        // refusing existing dirs); the root must exist by then.
        std::fs::create_dir_all(&to)?;
        let started_at_unix_ns = unix_nanos();
        let backup_id = format!("ob-{started_at_unix_ns}");
        let cancel = {
            let mut record = self.record.lock();
            if record.running {
                return Err(StartBackupError::AlreadyRunning);
            }
            let cancel = CancelFlag::default();
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
            };
            *self.backup_id.lock() = Some(backup_id.clone());
            cancel
        };
        let job = RunJob {
            db,
            data_dir: self.data_dir.clone(),
            runtime: self.runtime.clone(),
            permit: Arc::clone(&self.permit),
            cancel,
            to: to.clone(),
            run_state: self.record.lock().run.clone(),
            record: Arc::clone(&self.record),
        };
        // §4.8 metrics hook: the run counts as started once registration
        // succeeded; a failed spawn is recorded as a preparing-stage
        // failure so `shamir_backup_active` cannot stick at 1.
        backup_metrics::note_started();
        let spawned = std::thread::Builder::new()
            .name("online-backup".to_string())
            .spawn(move || job.run());
        if let Err(error) = spawned {
            backup_metrics::note_finished(true, BackupStage::Preparing, 0, Duration::ZERO);
            return Err(StartBackupError::Io(io::Error::other(error.to_string())));
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

/// Everything the runner thread owns for one backup.
struct RunJob {
    db: Arc<ShamirDb>,
    data_dir: PathBuf,
    runtime: Handle,
    to: PathBuf,
    permit: Arc<BackupPermit>,
    cancel: CancelFlag,
    run_state: Arc<PortRunState>,
    record: Arc<Mutex<RunRecord>>,
}

impl RunJob {
    /// The blocking-thread body: permit, protocol, record update. Never
    /// holds the record lock across the protocol call.
    fn run(self) {
        let Self {
            db,
            data_dir,
            runtime,
            to,
            permit,
            cancel,
            run_state,
            record,
        } = self;
        let started_at = Instant::now();
        let Some(permit_guard) = permit.try_acquire() else {
            // Defensive: the registry gate makes this unreachable today;
            // record a clean failure instead of racing the protocol.
            let mut record = record.lock();
            record.running = false;
            record.last_error = Some("another online backup holds the permit".to_string());
            backup_metrics::note_finished(true, BackupStage::Preparing, 0, started_at.elapsed());
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
        let cap = SealRecopyCap {
            max_bytes: Some(SEAL_RECOPY_MAX_BYTES),
            max_seconds: Some(SEAL_RECOPY_MAX_SECONDS),
        };
        let outcome = run_online_backup(
            &port,
            &data_dir,
            &to,
            &cancel,
            &mut progress,
            &options,
            &cap,
        );
        // Owner-side final progress: the protocol only notes zeroed
        // per-repo entries (step B) and exposes no progress callback, so
        // the run's hashed bytes/files would otherwise stay invisible in
        // `status()` after `Done`. Attribute the snapshot (which the
        // manifest describes per repo); recorded under a short lock below.
        if let Ok(report) = &outcome {
            for (repo, bytes_hashed, files_hashed) in final_per_repo_progress(report) {
                progress.note_repo_progress(&repo, bytes_hashed, files_hashed);
            }
        }
        let snapshot_count = run_state.snapshot_count();
        let repos_created_during = live_repo_count(&db, &runtime).saturating_sub(snapshot_count);
        // Release the permit before publishing idle, or a new run can be
        // refused.
        drop(permit_guard);
        let mut record = record.lock();
        record.running = false;
        record.progress = progress.clone();
        record.repos_created_during = repos_created_during;
        record.dest_dir = outcome.as_ref().ok().map(|report| report.dest_dir.clone());
        record.last_error = outcome.err().map(|error| error.to_string());
        // §4.8 metrics hook: run counters, copied bytes (the protocol's
        // hashed-byte counters) and the last run duration. The failure
        // stage is where the protocol recorded the failure.
        let bytes: u64 = progress.per_repo.iter().map(|repo| repo.bytes_hashed).sum();
        let failed = progress.stage == BackupStage::Failed;
        let stage = if failed {
            progress.failed_stage.unwrap_or(progress.stage)
        } else {
            progress.stage
        };
        backup_metrics::note_finished(failed, stage, bytes, started_at.elapsed());
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

/// Final per-repo hashed `(repo, bytes, files)` for a finished run,
/// derived owner-side from the snapshot manifest the protocol just wrote:
/// each captured repo is attributed the snapshot files under its store
/// dir and its WAL dir. Files outside every repo's subtree (residual
/// pass, system store) stay unattributed. Best-effort: on any read or
/// decode gap an empty result is returned and the repos keep the zeroed
/// entries the protocol noted in step B.
fn final_per_repo_progress(report: &BackupReport) -> Vec<(String, u64, u64)> {
    let parsed = std::fs::read_to_string(report.dest_dir.join("manifest.json"))
        .ok()
        .and_then(|raw| manifest::decode(&raw).ok());
    let Some(manifest_data) = parsed else {
        return Vec::new();
    };
    let Some(consistency) = manifest_data.consistency.as_ref() else {
        return Vec::new();
    };
    // WAL destination dirs -> owning repo (the manifest's WAL section
    // already records that attribution).
    let wal_repo = |path: &str| -> Option<&str> {
        manifest_data
            .wal
            .as_ref()?
            .dirs
            .iter()
            .find(|dir| path.starts_with(&format!("{}/", dir.dir)))
            .map(|dir| dir.repo.as_str())
    };
    let mut totals: Vec<(String, u64, u64)> = consistency
        .per_repo
        .keys()
        .map(|repo| (repo.clone(), 0, 0))
        .collect();
    for file in &manifest_data.files {
        let repo = consistency
            .per_repo
            .iter()
            .find(|(_, entry)| file.path.starts_with(&format!("{}/", entry.store_dir)))
            .map(|(repo, _)| repo.as_str())
            .or_else(|| wal_repo(&file.path));
        if let Some(repo) = repo {
            if let Some(entry) = totals.iter_mut().find(|(name, _, _)| name == repo) {
                entry.1 += file.size_bytes;
                entry.2 += 1;
            }
        }
    }
    totals
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

/// Offline-backup-equivalent destination validation: absolute, and its
/// nearest existing canonical ancestor must not be inside (or equal to) the
/// source data dir.
fn validate_destination(to: &Path, data_dir: &Path) -> Result<(), StartBackupError> {
    if !to.is_absolute() {
        return Err(StartBackupError::InvalidDestination(format!(
            "destination must be absolute: {}",
            to.display()
        )));
    }
    let to_canonical = canonicalize_nearest_existing_ancestor(to)?;
    let source_canonical = data_dir.canonicalize().map_err(|error| {
        StartBackupError::InvalidDestination(format!(
            "canonicalize source {}: {error}",
            data_dir.display()
        ))
    })?;
    if to_canonical.starts_with(&source_canonical) {
        return Err(StartBackupError::InvalidDestination(format!(
            "destination is inside (or equal to) the source data dir: {}",
            to.display()
        )));
    }
    Ok(())
}

/// Canonicalize `path` if it exists, else walk up to the nearest existing
/// ancestor and append the non-canonical suffix (offline precedent).
fn canonicalize_nearest_existing_ancestor(path: &Path) -> Result<PathBuf, StartBackupError> {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        if current.exists() {
            let mut canonical = current.canonicalize().map_err(|error| {
                StartBackupError::InvalidDestination(format!(
                    "canonicalize {}: {error}",
                    current.display()
                ))
            })?;
            for component in suffix.iter().rev() {
                canonical.push(component);
            }
            return Ok(canonical);
        }
        let Some(parent) = current.parent() else {
            return Err(StartBackupError::InvalidDestination(format!(
                "no existing ancestor for {}",
                path.display()
            )));
        };
        suffix.push(current.file_name().unwrap_or_default().to_os_string());
        current = parent;
    }
}
