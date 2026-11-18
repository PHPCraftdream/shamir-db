//! Online backup protocol (design `online-backup.md` 4.2): steps A-G over
//! the port, WAL copied before stores, a second pass at the seal barrier,
//! the manifest written last with fsync, self-verification before
//! success. Cancellation is checked at every stage boundary and between
//! files; a failed or cancelled run removes the destination directory and
//! releases the pins.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[cfg(test)]
use super::copy_engine::test_seams;
use super::copy_engine::{self, CopiedFile, CopyError, CopyOptions, CopyPassResult};
use super::manifest::{self, ConsistencySection, WalDirEntry, WalSection, WalSegmentEntry};
use super::progress::{BackupProgress, BackupStage, CancelFlag};
use super::wal_capture::{CapturedPoint, OnlineBackupPort, PortError, RepoSource};
use crate::sync::durability::backup::{utc_timestamp, verify_manifest, ManifestFileEntry};

/// Directory skipped at the top level of the data-dir copy (offline
/// backup precedent).
const SKIP_TOP_LEVEL_DIRS: &[&str] = &["cache"];

/// One in-process backup permit (design 4.4 / D9): a second concurrent
/// online backup is refused.
#[derive(Debug, Default)]
pub struct BackupPermit(AtomicBool);

impl BackupPermit {
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub fn try_acquire(&self) -> Option<BackupPermitGuard<'_>> {
        self.0
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| BackupPermitGuard(self))
    }
}

/// Held permit; releases on drop.
#[derive(Debug)]
pub struct BackupPermitGuard<'a>(&'a BackupPermit);

impl Drop for BackupPermitGuard<'_> {
    fn drop(&mut self) {
        self.0 .0.store(false, Ordering::Release);
    }
}

/// Failure causes of an online backup run.
#[derive(Debug, thiserror::Error)]
pub enum OnlineBackupFailure {
    #[error(transparent)]
    Port(#[from] PortError),
    #[error(transparent)]
    Copy(#[from] CopyError),
    #[error(transparent)]
    Manifest(#[from] manifest::ManifestError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("snapshot self-verification failed: {0}")]
    Verify(String),
    #[error("seal re-copy cap exceeded: {0}")]
    RecopyCap(String),
}

/// Fail-closed cap (design "Step E re-copy versus R3" / Q12) bounding each
/// repo's seal re-copy, after which the backup FAILS CLOSED (`Failed` at
/// `BackupStage::Sealing`, `OnlineBackupFailure::RecopyCap`) instead of
/// blocking writers unboundedly. `None` means unlimited (bytes: the whole
/// store; clock: unbounded), so existing callers keep working — production
/// callers should always set both (the re-copy runs while writers'
/// visibility is paused, so an uncapped large store stalls commits).
/// The clock cap is enforced at the per-repo re-copy completion and stage
/// boundaries only: the copy engine has no per-file deadline support. It is
/// a wall-clock bound with sub-second resolution (a cap of 0 seconds still
/// fails closed on a fast run).
#[derive(Debug, Clone, Default)]
pub struct SealRecopyCap {
    /// Maximum bytes a single repo's seal re-copy (WAL dir + store dir)
    /// may copy.
    pub max_bytes: Option<u64>,
    /// Maximum wall-clock seconds (sub-second resolution) from step E entry
    /// past a repo's seal re-copy completion.
    pub max_seconds: Option<u64>,
}

/// Protocol failure carrying the stage it failed at.
#[derive(Debug, thiserror::Error)]
pub enum OnlineBackupError {
    #[error("another online backup is in progress")]
    Busy,
    #[error("backup cancelled at stage {stage:?}")]
    Cancelled { stage: BackupStage },
    #[error("backup failed at stage {stage:?}")]
    Failed {
        stage: BackupStage,
        #[source]
        source: OnlineBackupFailure,
    },
}

impl OnlineBackupError {
    pub fn stage(&self) -> BackupStage {
        match self {
            OnlineBackupError::Busy => BackupStage::Preparing,
            OnlineBackupError::Cancelled { stage } => *stage,
            OnlineBackupError::Failed { stage, .. } => *stage,
        }
    }
}

/// Successful run summary.
#[derive(Debug, Clone)]
pub struct BackupReport {
    pub dest_dir: PathBuf,
    pub files_hashed: u64,
    pub total_bytes: u64,
}

fn failed(stage: BackupStage, source: impl Into<OnlineBackupFailure>) -> OnlineBackupError {
    OnlineBackupError::Failed {
        stage,
        source: source.into(),
    }
}

/// Destination directory for a repo's WAL: the wal dir's path relative to
/// the data dir, or `<repo>.shamirwal` as a fallback for out-of-tree WALs.
fn wal_dest_dir(source_root: &Path, dest_dir: &Path, repo: &RepoSource) -> PathBuf {
    match repo
        .wal_dir
        .as_ref()
        .and_then(|wal| wal.strip_prefix(source_root).ok())
    {
        Some(rel) => dest_dir.join(rel),
        None => dest_dir.join(format!("{}.shamirwal", repo.name)),
    }
}

fn rel_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// A copy error as a stage-tagged protocol error; a cancellation observed
/// between files is `Cancelled`, not a failure.
fn copy_failed(stage: BackupStage, error: CopyError) -> OnlineBackupError {
    match error {
        CopyError::Cancelled => OnlineBackupError::Cancelled { stage },
        other => failed(stage, other),
    }
}

/// One copy pass mapped onto a stage-tagged protocol error.
fn run_pass(
    source: &Path,
    dest: &Path,
    skip: &'static [&'static str],
    cancel: &CancelFlag,
    options: &CopyOptions,
    stage: BackupStage,
) -> Result<CopyPassResult, OnlineBackupError> {
    copy_engine::run_copy_pass(source, dest, skip, &[], cancel, options)
        .map_err(|error| copy_failed(stage, error))
}

/// WAL copy passes for every file-backed repo, WAL before stores
/// (design 4.2 step C). Returns (repo name, snapshot-relative dir,
/// files relative to the WAL dir).
fn copy_wal_dirs(
    repos: &[RepoSource],
    source_root: &Path,
    dest_dir: &Path,
    cancel: &CancelFlag,
    options: &CopyOptions,
    stage: BackupStage,
) -> Result<Vec<(String, String, Vec<CopiedFile>)>, OnlineBackupError> {
    let mut copied = Vec::new();
    for repo in repos {
        if let Some(wal) = &repo.wal_dir {
            let dest_wal = wal_dest_dir(source_root, dest_dir, repo);
            let result = run_pass(wal, &dest_wal, &[], cancel, options, stage)?;
            let dir_rel = dest_wal
                .strip_prefix(dest_dir)
                .map(rel_slash)
                .unwrap_or_else(|_| format!("{}.shamirwal", repo.name));
            copied.push((repo.name.clone(), dir_rel, result.files));
        }
    }
    Ok(copied)
}

/// Build one repo's `wal` manifest entry from the final WAL pass files
/// (paths relative to the WAL dir).
fn build_wal_dir_entry(repo: &str, dir_rel: &str, files: &[CopiedFile]) -> WalDirEntry {
    let mut segments = Vec::new();
    for file in files {
        let name = file.rel_path.rsplit('/').next().unwrap_or(&file.rel_path);
        let Some(stem) = name.strip_suffix(".wal") else {
            continue;
        };
        let Ok(seq) = stem.parse::<u64>() else {
            continue;
        };
        let sidecar_rel = format!(
            "{}.meta",
            file.rel_path.strip_suffix(".wal").unwrap_or(&file.rel_path)
        );
        let sidecar = files
            .iter()
            .find(|candidate| candidate.rel_path == sidecar_rel);
        // Segment and sidecar paths are snapshot-relative: the verifier
        // resolves them against the snapshot root.
        segments.push(WalSegmentEntry {
            seq,
            file: format!("{dir_rel}/{}", file.rel_path),
            sidecar: sidecar.map(|entry| format!("{dir_rel}/{}", entry.rel_path)),
            sha256: file.sha256.clone(),
            size: file.size_bytes,
        });
    }
    segments.sort_by_key(|entry| entry.seq);
    WalDirEntry {
        repo: repo.to_string(),
        dir: dir_rel.to_string(),
        segments,
    }
}

/// Self-verification before success (design 4.2 step F): re-hash every
/// file the manifest describes against the snapshot on disk.
fn self_verify(dest_dir: &Path, raw: &str) -> Result<(), OnlineBackupFailure> {
    let decoded = manifest::decode(raw)?;
    for entry in &decoded.files {
        let path = dest_dir.join(&entry.path);
        let (sha256, size) = copy_engine::hash_file(&path)
            .map_err(|error| OnlineBackupFailure::Verify(format!("{}: {error}", entry.path)))?;
        if sha256 != entry.sha256 || size != entry.size_bytes {
            return Err(OnlineBackupFailure::Verify(format!(
                "{}: hash mismatch",
                entry.path
            )));
        }
    }
    // The same verifier `verify` runs on a finished snapshot (WAL listing,
    // unmanifested files), so a snapshot that reports success also verifies.
    verify_manifest(dest_dir).map_err(|error| OnlineBackupFailure::Verify(error.to_string()))?;
    Ok(())
}

/// Per-repo seal re-copy outputs whose final hashes define the snapshot
/// (design 4.2 step E): consistency points, per-repo WAL dirs, repo store
/// files (snapshot-relative paths) and the residual store pass covering
/// everything outside the repo store/WAL dirs (system store, top-level
/// files).
#[derive(Default)]
struct SealRecopy {
    points: Vec<(String, CapturedPoint)>,
    wal_final: Vec<(String, String, Vec<CopiedFile>)>,
    store_files: Vec<CopiedFile>,
    residual: CopyPassResult,
}

fn recopy_bytes(files: &[CopiedFile]) -> u64 {
    files.iter().map(|file| file.size_bytes).sum()
}

fn clock_cap_exceeded(cap: &SealRecopyCap, started: &Instant) -> bool {
    // Sub-second resolution: a cap of 0 seconds fails closed even on a fast
    // run. Checked at each repo's re-copy completion and stage boundaries.
    cap.max_seconds
        .is_some_and(|max| started.elapsed().as_secs_f64() > max as f64)
}

/// Store-destination directory relative to the snapshot root: the store
/// dir's path relative to the data dir, or the captured point's reported
/// relative store dir as a fallback.
fn store_dest_rel(source_root: &Path, repo: &RepoSource, point: &CapturedPoint) -> String {
    repo.store_dir
        .strip_prefix(source_root)
        .map(rel_slash)
        .unwrap_or_else(|_| point.store_dir.clone())
}

/// Run the online backup protocol steps A-G (design 4.2).
pub fn run_online_backup(
    port: &dyn OnlineBackupPort,
    source_root: &Path,
    to: &Path,
    cancel: &CancelFlag,
    progress: &mut BackupProgress,
    options: &CopyOptions,
    cap: &SealRecopyCap,
) -> Result<BackupReport, OnlineBackupError> {
    let dir = to.join(utc_timestamp());
    fs::create_dir(&dir).map_err(|error| failed(BackupStage::Preparing, error))?;
    let result = run_steps(port, source_root, &dir, cancel, progress, options, cap);
    match result {
        Ok(report) => {
            port.release_pins();
            progress.set_stage(BackupStage::Done);
            Ok(report)
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&dir);
            port.release_pins();
            progress.record_failure(error.stage(), &error);
            Err(error)
        }
    }
}

fn run_steps(
    port: &dyn OnlineBackupPort,
    source_root: &Path,
    dest_dir: &Path,
    cancel: &CancelFlag,
    progress: &mut BackupProgress,
    options: &CopyOptions,
    cap: &SealRecopyCap,
) -> Result<BackupReport, OnlineBackupError> {
    // A. Prepare: catalogue snapshot, pins, atomic destination create.
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Preparing,
        });
    }
    let repos = port
        .catalogue()
        .map_err(|error| failed(BackupStage::Preparing, error))?;
    // WAL dirs are copied by their own passes (before stores); the store
    // pass must not copy them again, or later bytes would overwrite them.
    let wal_dirs: Vec<PathBuf> = repos
        .iter()
        .filter_map(|repo| repo.wal_dir.clone())
        .collect();
    port.acquire_pins()
        .map_err(|error| failed(BackupStage::Preparing, error))?;

    // B. Capture barrier: per-repo flushed positions + system store.
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Captured,
        });
    }
    for repo in &repos {
        let _point = port
            .capture_point(&repo.name)
            .map_err(|error| failed(BackupStage::Captured, error))?;
        progress.note_repo_progress(&repo.name, 0, 0);
    }
    let system_store_version = port
        .capture_system_store()
        .map_err(|error| failed(BackupStage::Captured, error))?;

    // C+D. Copy WAL first, then the whole data dir (writers running).
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Copying,
        });
    }
    copy_wal_dirs(
        &repos,
        source_root,
        dest_dir,
        cancel,
        options,
        BackupStage::Copying,
    )?;
    // Test-only seam: raise cancellation exactly at the C→D boundary
    // (after the WAL passes, before the first store pass — a point no
    // copy-pass hook can reach); the store pass's per-file loop top is
    // the existing cancellation check that observes it.
    #[cfg(test)]
    test_seams::fire_before_boundary(BackupStage::Copying);
    copy_engine::run_copy_pass(
        source_root,
        dest_dir,
        SKIP_TOP_LEVEL_DIRS,
        &wal_dirs,
        cancel,
        options,
    )
    .map_err(|error| copy_failed(BackupStage::Copying, error))?;

    // E. Seal (design "Step 1d decisions" (a)/(b), Q12): per repo —
    // seal_point, THAT repo's seal re-copy (WAL dir then store dir) whose
    // final hashes define the snapshot, then end_seal; the seal hold lives
    // exactly through its re-copy. The re-copy runs while writers'
    // visibility is paused (drain pause survives seal_point), bounded by
    // `cap`: exceeding the byte cap fails closed; the clock cap is checked
    // at each repo's re-copy completion and at the stage boundary. Any
    // failure or cancellation ends every seal still open (`end_seal` is
    // idempotent) before propagating.
    // Test-only seam: raise cancellation exactly at the D→E boundary
    // (a point no copy-pass hook can reach); must stay before the
    // existing cancellation check below.
    #[cfg(test)]
    test_seams::fire_before_boundary(BackupStage::Sealing);
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Sealing,
        });
    }
    let mut sealed_open: Vec<String> = Vec::new();
    let started = Instant::now();
    let cap_failed = |what: String| -> OnlineBackupError {
        failed(BackupStage::Sealing, OnlineBackupFailure::RecopyCap(what))
    };
    let recopy = (|| -> Result<SealRecopy, OnlineBackupError> {
        let mut recopy = SealRecopy::default();
        // Repo store/WAL dirs are re-copied per repo below; the residual
        // store pass at the end must not copy them again.
        let mut exclude = wal_dirs.clone();
        for repo in &repos {
            exclude.push(repo.store_dir.clone());
        }
        for repo in &repos {
            // The repo counts as seal-open from the attempt on:
            // `end_seal` is documented idempotent, so ending a repo whose
            // seal never opened is a no-op; ending one whose hold IS open
            // on an error path is mandatory (fail-closed).
            sealed_open.push(repo.name.clone());
            let point = port
                .seal_point(&repo.name)
                .map_err(|error| failed(BackupStage::Sealing, error))?;
            let mut bytes = 0u64;
            if repo.wal_dir.is_some() {
                let wal = copy_wal_dirs(
                    std::slice::from_ref(repo),
                    source_root,
                    dest_dir,
                    cancel,
                    options,
                    BackupStage::Sealing,
                )?;
                bytes += wal
                    .iter()
                    .map(|(_, _, files)| recopy_bytes(files))
                    .sum::<u64>();
                recopy.wal_final.extend(wal);
            }
            let store_rel = store_dest_rel(source_root, repo, &point);
            let store = run_pass(
                &repo.store_dir,
                &dest_dir.join(&store_rel),
                &[],
                cancel,
                options,
                BackupStage::Sealing,
            )?;
            bytes += recopy_bytes(&store.files);
            for file in store.files {
                recopy.store_files.push(CopiedFile {
                    rel_path: format!("{store_rel}/{}", file.rel_path),
                    sha256: file.sha256,
                    size_bytes: file.size_bytes,
                });
            }
            if let Some(max) = cap.max_bytes {
                if bytes > max {
                    return Err(cap_failed(format!(
                        "repo {} copied {bytes} bytes, cap {max}",
                        repo.name
                    )));
                }
            }
            if clock_cap_exceeded(cap, &started) {
                return Err(cap_failed(format!(
                    "repo {} seal re-copy exceeded {}s",
                    repo.name,
                    cap.max_seconds.unwrap_or_default()
                )));
            }
            port.end_seal(&repo.name);
            sealed_open.retain(|open| open != &repo.name);
            recopy.points.push((repo.name.clone(), point));
        }
        recopy.residual = copy_engine::run_copy_pass(
            source_root,
            dest_dir,
            SKIP_TOP_LEVEL_DIRS,
            &exclude,
            cancel,
            options,
        )
        .map_err(|error| copy_failed(BackupStage::Sealing, error))?;
        if clock_cap_exceeded(cap, &started) {
            return Err(cap_failed(format!(
                "seal re-copy exceeded {}s",
                cap.max_seconds.unwrap_or_default()
            )));
        }
        Ok(recopy)
    })();
    // Failure/cancellation inside the seal loop: end every hold still open.
    for repo in &sealed_open {
        port.end_seal(repo);
    }
    let recopy = recopy?;

    // F. Verify + manifest: manifest from the final pass, written last
    // with fsync, then self-verification.
    // Test-only seam: raise cancellation exactly at the E→F boundary
    // (all seal holds already ended); must stay before the existing
    // cancellation check below.
    #[cfg(test)]
    test_seams::fire_before_boundary(BackupStage::Verifying);
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Verifying,
        });
    }
    let per_repo: BTreeMap<String, _> = recopy
        .points
        .iter()
        .map(|(repo, point)| (repo.clone(), point.to_consistency(repo)))
        .collect();
    let consistency = ConsistencySection {
        per_repo,
        system_store_version,
    };
    let dirs = recopy
        .wal_final
        .iter()
        .map(|(repo, dir_rel, files)| build_wal_dir_entry(repo, dir_rel, files))
        .collect();
    let wal = WalSection { dirs };
    let mut files: Vec<ManifestFileEntry> = recopy
        .residual
        .files
        .iter()
        .map(|file| ManifestFileEntry {
            path: file.rel_path.clone(),
            sha256: file.sha256.clone(),
            size_bytes: file.size_bytes,
        })
        .collect();
    for file in &recopy.store_files {
        files.push(ManifestFileEntry {
            path: file.rel_path.clone(),
            sha256: file.sha256.clone(),
            size_bytes: file.size_bytes,
        });
    }
    for (_, dir_rel, wal_files) in &recopy.wal_final {
        for file in wal_files {
            files.push(ManifestFileEntry {
                path: format!("{dir_rel}/{}", file.rel_path),
                sha256: file.sha256.clone(),
                size_bytes: file.size_bytes,
            });
        }
    }
    let total_bytes = files.iter().map(|entry| entry.size_bytes).sum::<u64>();
    let files_hashed = files.len() as u64;
    let copied_stamp_epoch = match crate::format_stamp::inspect(dest_dir) {
        crate::format_stamp::StampState::Valid(stamp) => Some(stamp.format_epoch()),
        _ => None,
    };
    let manifest = manifest::build_v2(
        progress.started_at_unix_ns,
        files,
        copied_stamp_epoch,
        Some(env!("CARGO_PKG_VERSION").to_string()),
        consistency,
        wal,
    );
    let encoded =
        manifest::encode(&manifest).map_err(|error| failed(BackupStage::Verifying, error))?;
    manifest::write_manifest(dest_dir, &encoded)
        .map_err(|error| failed(BackupStage::Verifying, error))?;
    manifest::write_backup_meta(dest_dir).map_err(|error| failed(BackupStage::Verifying, error))?;
    let raw = fs::read_to_string(dest_dir.join("manifest.json"))
        .map_err(|error| failed(BackupStage::Verifying, error))?;
    self_verify(dest_dir, &raw).map_err(|error| failed(BackupStage::Verifying, error))?;
    // Test-only seam: observe the point after successful
    // self-verification, immediately before Done. Deliberately NO
    // cancellation check here — production keeps finishing the run: the
    // snapshot is already verified and published as a unit, and there is
    // no cancellation cleanup possible at this point (the destination is
    // final and about to be reported Done). A cancellation raised by the
    // hook still ends Done.
    #[cfg(test)]
    test_seams::fire_before_boundary(BackupStage::Done);

    Ok(BackupReport {
        dest_dir: dest_dir.to_path_buf(),
        files_hashed,
        total_bytes,
    })
}
