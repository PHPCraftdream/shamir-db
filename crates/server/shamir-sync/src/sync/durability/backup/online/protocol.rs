//! Online backup protocol (design `online-backup.md` 4.2): steps A-G over
//! the port, WAL copied before stores, a second pass at the seal barrier,
//! the manifest written last with fsync, self-verification before
//! success. Cancellation is checked at every stage boundary and between
//! files; a failed or cancelled run attempts to remove its destination
//! directory and releases the pins — on the error return AND on an unwind, via the
//! RAII run cleanup armed right after the atomic destination create.
//! The server publishes the real phase through
//! the port's `begin_copy` (before the first WAL pass) and
//! `begin_verify` (at the start of F, after the cancellation check)
//! hooks; Sealing stays the reported stage through E including the
//! residual pass.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::super::offline::verify_writer_snapshot;
#[cfg(test)]
use super::copy_engine::test_seams;
use super::copy_engine::{self, CopiedFile, CopyError, CopyOptions, CopyPassResult, SealGate};
use super::manifest::{self, ConsistencySection, WalDirEntry, WalSection, WalSegmentEntry};
use super::progress::{BackupProgress, BackupStage, CancelFlag};
use super::snapshot_name;
use super::wal_capture::{CapturedPoint, OnlineBackupPort, PortError, RepoSource};
use crate::sync::durability::backup::ManifestFileEntry;

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
    /// The DESTINATION side could not be prepared: specifically, atomic
    /// creation of the snapshot child directory failed. Manifest and
    /// backup-metadata publication failures use
    /// [`OnlineBackupFailure::Publication`]. Kept apart from
    /// [`OnlineBackupFailure::Io`] so the scheduled reason names a
    /// DESTINATION failure (`destination-io-<kind>`) instead of looking like a
    /// generic io error; it carries no path, so the rendering stays path-free.
    #[error("destination io error: {source}")]
    Dest {
        #[source]
        source: std::io::Error,
    },
    /// The snapshot's PUBLICATION into the already-created snapshot child
    /// directory (the manifest and the backup-metadata file) failed: unlike
    /// [`OnlineBackupFailure::Dest`], which covers only atomic creation of
    /// that child, this variant includes the directory path. The scheduled
    /// vocabulary is unchanged (`destination-io-<kind>`).
    #[error("destination publication failed for {dir}: {source}")]
    Publication {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("snapshot self-verification failed: {0}")]
    Verify(String),
    #[error("seal re-copy cap exceeded: {0}")]
    RecopyCap(String),
}

/// Fail-closed cap (design "Step E re-copy versus R3" / Q12) bounding each
/// repo's seal re-copy, after which the backup FAILS CLOSED (`Failed` at
/// `BackupStage::Sealing`, `OnlineBackupFailure::RecopyCap`) instead of
/// blocking writers unboundedly. The seal re-copy runs while writers'
/// admission to the repo is closed (visibility paused via the open seal
/// hold), so the TIME cap is the real protection — production callers
/// should always set `max_seconds` (8s is the default guidance). The byte
/// cap is an optional extra safety net, not a production requirement:
/// `None` means unlimited (bytes: the whole store; clock: unbounded), so
/// existing callers keep working. The byte cap, when set, is enforced
/// DURING the re-copy by the copy engine's
/// cooperative gate: checked before each written chunk, cumulatively
/// across that repo's WAL + store passes INCLUDING retries, around the
/// fsyncs, with the sharing-retry sleeps clamped to the clock deadline.
/// The clock cap bounds the same gate (read/hash and writes alike);
/// the per-repo monotonic start is taken immediately before the seal
/// barrier, so the barrier counts as elapsed budget. A blocking syscall
/// can overrun the clock: enforcement is cooperative, never a detached
/// kill. The gate itself is checked once more before each repo's hold
/// is ended; no stage-wide completion cap remains. It is
/// a wall-clock bound with sub-second resolution (a cap of 0 seconds still
/// fails closed on a fast run).
#[derive(Debug, Clone, Default)]
pub struct SealRecopyCap {
    /// Maximum bytes a single repo's seal re-copy (WAL dir + store dir)
    /// may copy.
    pub max_bytes: Option<u64>,
    /// Maximum wall-clock seconds (sub-second resolution) from a repo's
    /// seal-re-copy start — taken immediately before `seal_point_before`,
    /// so the barrier is inside the budget — past that repo's re-copy
    /// completion.
    pub max_seconds: Option<u64>,
}

/// A failed run's partial-tree cleanup failure, reported SEPARATELY
/// from the primary error: cleanup attempts to remove the whole destination
/// run directory but can fail and leave it behind. This value is bounded and
/// path-free by construction — it carries only the `io::ErrorKind` of the
/// failed removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupFailure {
    pub kind: ErrorKind,
}

/// Protocol failure carrying the stage it failed at.
#[derive(Debug, thiserror::Error)]
pub enum OnlineBackupError {
    #[error("another online backup is in progress")]
    Busy,
    #[error("backup cancelled at stage {stage:?}")]
    Cancelled {
        stage: BackupStage,
        /// Failure of the best-effort removal attempt for the owned partial
        /// destination, if any; cleanup can fail and leave the directory.
        /// The cancellation stays the PRIMARY outcome.
        cleanup: Option<CleanupFailure>,
    },
    #[error("backup failed at stage {stage:?}")]
    Failed {
        stage: BackupStage,
        #[source]
        source: OnlineBackupFailure,
        /// Failure of the best-effort removal attempt for the owned partial
        /// destination, if any; cleanup can fail and leave the directory.
        /// `source` stays the PRIMARY error.
        cleanup: Option<CleanupFailure>,
    },
}

impl OnlineBackupError {
    pub fn stage(&self) -> BackupStage {
        match self {
            OnlineBackupError::Busy => BackupStage::Preparing,
            OnlineBackupError::Cancelled { stage, .. } => *stage,
            OnlineBackupError::Failed { stage, .. } => *stage,
        }
    }

    /// Attach the cleanup failure of the owned partial destination to a
    /// failed OR cancelled run; the primary outcome is unchanged. A no-op
    /// on `Busy` (it owns no destination).
    pub fn with_cleanup_failure(mut self, cleanup: CleanupFailure) -> Self {
        match &mut self {
            OnlineBackupError::Failed { cleanup: slot, .. }
            | OnlineBackupError::Cancelled { cleanup: slot, .. } => *slot = Some(cleanup),
            OnlineBackupError::Busy => {}
        }
        self
    }

    /// Stage plus a bounded reason: the rendering a SCHEDULED run stores
    /// and logs. Never contains a filesystem path and never the OS error
    /// text (no `Display` of a source chain, no `io::Error` message).
    pub fn stage_and_reason(&self) -> String {
        let mut rendered = format!("{}: {}", stage_word(self.stage()), self.reason_token());
        if let Some(cleanup) = self.cleanup() {
            rendered.push_str("; cleanup-failed-");
            rendered.push_str(&kind_token(cleanup.kind));
        }
        rendered
    }

    /// The failed partial-tree removal, when one was attached.
    fn cleanup(&self) -> Option<&CleanupFailure> {
        match self {
            OnlineBackupError::Failed { cleanup, .. }
            | OnlineBackupError::Cancelled { cleanup, .. } => cleanup.as_ref(),
            OnlineBackupError::Busy => None,
        }
    }

    /// The fixed reason token of this failure (see
    /// [`OnlineBackupError::stage_and_reason`]).
    fn reason_token(&self) -> String {
        match self {
            OnlineBackupError::Busy => "busy".to_string(),
            OnlineBackupError::Cancelled { .. } => "cancelled".to_string(),
            OnlineBackupError::Failed { source, .. } => failure_token(source),
        }
    }
}

/// Stage plus a bounded `io-<kind>` reason for a failure that has no
/// [`OnlineBackupError`] value (the scheduled spawn-failure path). Shares
/// the SCHEDULED vocabulary with [`OnlineBackupError::stage_and_reason`];
/// `kind_token` stays private here.
pub fn stage_and_io_reason(stage: BackupStage, kind: ErrorKind) -> String {
    format!("{}: io-{}", stage_word(stage), kind_token(kind))
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
        cleanup: None,
    }
}

/// Lowercase stage word of the path-free rendering (never `{:?}` of the
/// enum).
fn stage_word(stage: BackupStage) -> &'static str {
    match stage {
        BackupStage::Preparing => "preparing",
        BackupStage::Captured => "captured",
        BackupStage::Copying => "copying",
        BackupStage::Sealing => "sealing",
        BackupStage::Verifying => "verifying",
        BackupStage::Done => "done",
        BackupStage::Failed => "failed",
    }
}

/// Fixed reason token of a failure cause: a stage vocabulary, never an
/// OS message or a path.
fn failure_token(failure: &OnlineBackupFailure) -> String {
    match failure {
        OnlineBackupFailure::Port(_) => "port-failed".to_string(),
        OnlineBackupFailure::Manifest(_) => "manifest-failed".to_string(),
        OnlineBackupFailure::Verify(_) => "verify-failed".to_string(),
        OnlineBackupFailure::RecopyCap(_) => "recopy-cap-exceeded".to_string(),
        OnlineBackupFailure::Io(error) => format!("io-{}", kind_token(error.kind())),
        OnlineBackupFailure::Dest { source } | OnlineBackupFailure::Publication { source, .. } => {
            format!("destination-io-{}", kind_token(source.kind()))
        }
        OnlineBackupFailure::Copy(error) => copy_token(error),
    }
}

/// Fixed reason token of a copy failure, distinguishing a destination
/// error from a source (`io-`) error.
fn copy_token(error: &CopyError) -> String {
    match error {
        CopyError::Cancelled => "copy-cancelled".to_string(),
        CopyError::Io { source, .. } => format!("copy-io-{}", kind_token(source.kind())),
        CopyError::Dest { source, .. } => {
            format!("copy-destination-io-{}", kind_token(source.kind()))
        }
        CopyError::RecopyCap(_) => "copy-recopy-cap".to_string(),
        CopyError::AuditFilesUnstable { .. } => "copy-audit-files-unstable".to_string(),
        CopyError::AuditListing { source } => format!("copy-io-{}", kind_token(source.kind())),
        CopyError::AuditArchiveLost => "copy-audit-archive-lost".to_string(),
    }
}

/// Stable kebab-case spelling of an `io::ErrorKind`'s `Debug` name
/// (`NotFound` -> `not-found`, `PermissionDenied` ->
/// `permission-denied`); underscores are kept.
fn kind_token(kind: ErrorKind) -> String {
    let mut token = String::new();
    for (index, character) in format!("{kind:?}").chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                token.push('-');
            }
            token.push(character.to_ascii_lowercase());
        } else {
            token.push(character);
        }
    }
    token
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
/// between files is `Cancelled`, a tripped seal gate is the typed
/// `RecopyCap` failure (always at `Sealing`), everything else fails the
/// stage.
fn copy_failed(stage: BackupStage, error: CopyError) -> OnlineBackupError {
    match error {
        CopyError::Cancelled => OnlineBackupError::Cancelled {
            stage,
            cleanup: None,
        },
        CopyError::RecopyCap(message) => failed(
            BackupStage::Sealing,
            OnlineBackupFailure::RecopyCap(message),
        ),
        other => failed(stage, other),
    }
}

/// A DESTINATION-side publication failure (the snapshot child's create,
/// the manifest and backup-metadata files) as a stage-tagged protocol
/// error: it is `OnlineBackupFailure::Dest`, never a generic io error, so
/// `stage_and_reason` names the destination (`destination-io-<kind>`).
fn dest_failed(stage: BackupStage, error: std::io::Error) -> OnlineBackupError {
    failed(stage, OnlineBackupFailure::Dest { source: error })
}

/// A PUBLICATION failure inside `dir` (the manifest, the backup-metadata
/// file) as a stage-tagged protocol error: it names the destination
/// DIRECTORY, so the operator sees which directory could not take the
/// published bytes. The scheduled token is the same destination vocabulary
/// (`destination-io-<kind>`).
fn publication_failed(stage: BackupStage, dir: &Path, error: std::io::Error) -> OnlineBackupError {
    failed(
        stage,
        OnlineBackupFailure::Publication {
            dir: dir.to_path_buf(),
            source: error,
        },
    )
}

/// One copy pass's shared context (keeps argument lists short): the
/// stage tag, cancellation, retry options and — for the seal re-copy
/// passes — the cooperative gate plus the destination digests
/// authenticated outside the hold.
struct PassCtx<'a> {
    cancel: &'a CancelFlag,
    options: &'a CopyOptions,
    stage: BackupStage,
    gate: Option<&'a SealGate>,
    preauth: &'a BTreeMap<String, (String, u64)>,
}

impl PassCtx<'_> {
    /// One copy pass mapped onto a stage-tagged protocol error.
    fn run(
        &self,
        source: &Path,
        dest: &Path,
        skip: &'static [&'static str],
    ) -> Result<CopyPassResult, OnlineBackupError> {
        let result = match self.gate {
            Some(gate) => copy_engine::run_copy_pass_gated(
                source,
                dest,
                self.cancel,
                self.options,
                gate,
                self.preauth,
            ),
            None => copy_engine::run_copy_pass(source, dest, skip, &[], self.cancel, self.options),
        };
        result.map_err(|error| copy_failed(self.stage, error))
    }
}

/// WAL copy passes for every file-backed repo, WAL before stores
/// (design 4.2 step C). Returns (repo name, snapshot-relative dir,
/// files relative to the WAL dir).
fn copy_wal_dirs(
    ctx: &PassCtx<'_>,
    repos: &[RepoSource],
    source_root: &Path,
    dest_dir: &Path,
) -> Result<Vec<(String, String, Vec<CopiedFile>)>, OnlineBackupError> {
    let mut copied = Vec::new();
    for repo in repos {
        if let Some(wal) = &repo.wal_dir {
            let dest_wal = wal_dest_dir(source_root, dest_dir, repo);
            let result = ctx.run(wal, &dest_wal, &[])?;
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
    // Unleased and path-exact: the writer cannot lease the child it is
    // still publishing, and no sibling of it is unleased here.
    verify_writer_snapshot(dest_dir)
        .map_err(|error| OnlineBackupFailure::Verify(error.to_string()))?;
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

/// RAII per-repo seal hold: spans the seal attempt through BOTH re-copy
/// passes and is ended on EVERY exit path (failure, cap trip,
/// cancellation, unwind) before the next repo and before any output
/// cleanup. `end_seal` is idempotent, so an end at drop for a hold that
/// never opened is a no-op (fail-closed). There is NO explicit-release
/// path: `drop` invokes `end_seal` exactly once per repo on every path,
/// success included (the protocol's seal-event ordering shows exactly
/// one `end_seal` per repo).
struct SealGuard<'a> {
    port: &'a dyn OnlineBackupPort,
    repo: &'a str,
}

impl Drop for SealGuard<'_> {
    fn drop(&mut self) {
        self.port.end_seal(self.repo);
    }
}

/// RAII cleanup of one run's owned resources: the destination directory
/// created atomically in `run_online_backup` and the run's pins. Its
/// `Drop` — on error return AND unwind — removes ONLY the owned directory
/// (a pre-existing destination tree is never touched) and releases the
/// pins exactly once, so nothing leaks past a panic and there is no
/// explicit release that could double-panic; the success path only sets
/// `keep_dir` before dropping the guard. A failed removal is stored in
/// `cleanup_failure`, which `run_online_backup` attaches to the PRIMARY
/// error before reporting it. The per-repo `SealGuard`s live inside
/// `run_steps` and drop before this guard, so no seal hold is still open
/// at cleanup.
struct RunCleanup<'a> {
    port: &'a dyn OnlineBackupPort,
    dir: &'a Path,
    keep_dir: bool,
    /// Kind of a failed removal of the owned partial directory; read by
    /// `run_online_backup` after the guard drops.
    cleanup_failure: &'a Cell<Option<CleanupFailure>>,
}

impl Drop for RunCleanup<'_> {
    fn drop(&mut self) {
        if !self.keep_dir {
            #[cfg(test)]
            let removed = test_seams::fire_remove_dir_all(self.dir)
                .unwrap_or_else(|| fs::remove_dir_all(self.dir));
            #[cfg(not(test))]
            let removed = fs::remove_dir_all(self.dir);
            if let Err(error) = removed {
                self.cleanup_failure
                    .set(Some(CleanupFailure { kind: error.kind() }));
            }
        }
        self.port.release_pins();
    }
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
    let dir = to.join(snapshot_name());
    // Atomic create: on failure nothing is owned and nothing is deleted —
    // a pre-existing destination tree stays untouched; no cleanup guard is
    // armed, so there is nothing to drop and no pins to release (they are
    // acquired only inside `run_steps`). The failure is still RECORDED in
    // the progress like every other error path, so the registry never
    // publishes a bare `Preparing` without a failed stage. A failure here
    // is a DESTINATION failure: the snapshot child is the protocol's own
    // output, never the source.
    if let Err(error) = fs::create_dir(&dir) {
        let error = dest_failed(BackupStage::Preparing, error);
        progress.record_failure(error.stage(), &error);
        return Err(error);
    }
    let cleanup_failure: Cell<Option<CleanupFailure>> = Cell::new(None);
    let mut cleanup = RunCleanup {
        port,
        dir: &dir,
        keep_dir: false,
        cleanup_failure: &cleanup_failure,
    };
    let result = run_steps(port, source_root, &dir, cancel, progress, options, cap);
    match result {
        Ok(report) => {
            // Verified: preserve the directory and let the guard's Drop
            // release the pins exactly once (the only release call on
            // any path).
            cleanup.keep_dir = true;
            drop(cleanup);
            progress.set_stage(BackupStage::Done);
            Ok(report)
        }
        Err(error) => {
            // The guard's Drop removes the owned partial directory and
            // releases the pins (also on the unwind path); the stage and
            // the error are preserved for the failure report. A failed
            // removal is attached SEPARATELY: the original error stays
            // primary.
            drop(cleanup);
            let error = match cleanup_failure.take() {
                Some(cleanup) => error.with_cleanup_failure(cleanup),
                None => error,
            };
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
            cleanup: None,
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
            cleanup: None,
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
            cleanup: None,
        });
    }
    // Publish the real `Copying` phase to the server before the first
    // WAL pass (capture barrier done, cancellation checked).
    port.begin_copy();
    copy_wal_dirs(
        &PassCtx {
            cancel,
            options,
            stage: BackupStage::Copying,
            gate: None,
            preauth: &BTreeMap::new(),
        },
        &repos,
        source_root,
        dest_dir,
    )?;
    // Test-only seam: raise cancellation exactly at the C→D boundary
    // (after the WAL passes, before the first store pass — a point no
    // copy-pass hook can reach); the store pass's per-file loop top is
    // the existing cancellation check that observes it.
    #[cfg(test)]
    test_seams::fire_before_boundary(BackupStage::Copying);
    copy_engine::run_data_dir_copy_pass(source_root, dest_dir, &wal_dirs, cancel, options)
        .map_err(|error| copy_failed(BackupStage::Copying, error))?;

    // E. Seal (design "Step 1d decisions" (a)/(b), Q12): per repo —
    // seal_point, THAT repo's seal re-copy (WAL dir then store dir) whose
    // final hashes define the snapshot, then end_seal; the seal hold lives
    // exactly through its re-copy, spanned by an RAII guard so EVERY exit
    // (failure, cap trip, cancellation, unwind) ends the hold before the
    // next repo and before any output cleanup (`end_seal` is idempotent).
    // The re-copy runs while writers' visibility is paused (drain pause
    // survives seal_point), bounded by `cap` cooperatively DURING the
    // work: the per-repo monotonic start is taken immediately before
    // `seal_point_before` so the barrier is inside the budget; the gate
    // travels through listing, per-chunk writes, sharing-retry sleeps
    // (clamped to the deadline) and the fsync surroundings of BOTH the
    // repo's WAL and store passes, and bytes are checked cumulatively
    // (retries included) BEFORE each chunk is written. A blocking syscall
    // can still overrun the deadline — the cap is cooperative, never a
    // detached kill. The residual pass holds NO gate.
    // Test-only seam: raise cancellation exactly at the D→E boundary
    // (a point no copy-pass hook can reach); must stay before the
    // existing cancellation check below.
    #[cfg(test)]
    test_seams::fire_before_boundary(BackupStage::Sealing);
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Sealing,
            cleanup: None,
        });
    }
    let recopy: SealRecopy = {
        let mut recopy = SealRecopy::default();
        // Repo store/WAL dirs are re-copied per repo below; the residual
        // store pass at the end must not copy them again.
        let mut exclude = wal_dirs.clone();
        for repo in &repos {
            exclude.push(repo.store_dir.clone());
        }
        for repo in &repos {
            // Destination digests are authenticated ONCE, outside the
            // seal hold (before the barrier) and CANCELLATION-ONLY —
            // this read-only pass does NOT consume the repo's writer-
            // block budget; the deadline is created only after it, so
            // the budget starts right before the actual seal. Each
            // destination tree authenticates into its OWN map (keys
            // are rel paths within that tree); a missing destination
            // root yields an empty map. Only when the destination dir
            // is not derivable pre-seal (out-of-tree store dir) is the
            // comparison left to hash-under-hold.
            let wal_preauth = if repo.wal_dir.is_some() {
                let dest_wal = wal_dest_dir(source_root, dest_dir, repo);
                copy_engine::authenticate_dest(&dest_wal, cancel, None)
                    .map_err(|error| copy_failed(BackupStage::Sealing, error))?
            } else {
                BTreeMap::new()
            };
            let store_dest_known = repo.store_dir.strip_prefix(source_root).ok().map(rel_slash);
            let store_preauth = match &store_dest_known {
                Some(store_rel) => {
                    copy_engine::authenticate_dest(&dest_dir.join(store_rel), cancel, None)
                        .map_err(|error| copy_failed(BackupStage::Sealing, error))?
                }
                None => BTreeMap::new(),
            };
            // RAII seal hold: declared immediately before the ACTUAL
            // seal attempt (after the read-only preauth), ended on
            // EVERY exit path — `drop` invokes the idempotent
            // `end_seal`, exactly once, on the success path too —
            // before the next repo and before any output cleanup.
            let guard = SealGuard {
                port,
                repo: &repo.name,
            };
            // Budget (clock) starts immediately before the actual seal
            // barrier call, so the barrier is inside the budget.
            let gate = SealGate::new(cap, Instant::now())
                .map_err(|error| copy_failed(BackupStage::Sealing, error))?;
            let wal_ctx = PassCtx {
                cancel,
                options,
                stage: BackupStage::Sealing,
                gate: Some(&gate),
                preauth: &wal_preauth,
            };
            let store_ctx = PassCtx {
                cancel,
                options,
                stage: BackupStage::Sealing,
                gate: Some(&gate),
                preauth: &store_preauth,
            };
            let point = match port.seal_point_before(&repo.name, gate.deadline(), cancel) {
                Ok(point) => point,
                Err(error) => {
                    // A typed cap/cancel outcome wins over the port's
                    // own error before it is forwarded.
                    if let Err(cap) = gate.check_clock() {
                        return Err(copy_failed(BackupStage::Sealing, cap));
                    }
                    if cancel.is_cancelled() {
                        return Err(OnlineBackupError::Cancelled {
                            stage: BackupStage::Sealing,
                            cleanup: None,
                        });
                    }
                    return Err(failed(BackupStage::Sealing, error));
                }
            };
            if repo.wal_dir.is_some() {
                let wal =
                    copy_wal_dirs(&wal_ctx, std::slice::from_ref(repo), source_root, dest_dir)?;
                recopy.wal_final.extend(wal);
            }
            let store_rel = store_dest_known
                .clone()
                .unwrap_or_else(|| point.store_dir.clone());
            let store = store_ctx.run(&repo.store_dir, &dest_dir.join(&store_rel), &[])?;
            for file in store.files {
                recopy.store_files.push(CopiedFile {
                    rel_path: format!("{store_rel}/{}", file.rel_path),
                    sha256: file.sha256,
                    size_bytes: file.size_bytes,
                });
            }
            // Final cooperative check before leaving the iteration for
            // the manifest mapping; the gate already fails closed
            // mid-pass, this covers the last loop of it.
            gate.check(cancel)
                .map_err(|error| copy_failed(BackupStage::Sealing, error))?;
            drop(guard);
            recopy.points.push((repo.name.clone(), point));
        }
        // Residual pass: NO gate (it holds no repo's seal) and NO cap
        // check — it is outside every hold; stage-boundary bounded.
        recopy.residual =
            copy_engine::run_data_dir_copy_pass(source_root, dest_dir, &exclude, cancel, options)
                .map_err(|error| copy_failed(BackupStage::Sealing, error))?;
        recopy
    };

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
            cleanup: None,
        });
    }
    port.begin_verify();
    port.validate_copied_data_dir(dest_dir)
        .map_err(|error| failed(BackupStage::Verifying, error))?;
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Verifying,
            cleanup: None,
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
    // A validation error wins via `?`; cancellation observed after successful validation wins over publication. This final check covers manifest preparation and encoding, which have no deterministic test seam.
    if cancel.is_cancelled() {
        return Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Verifying,
            cleanup: None,
        });
    }
    manifest::write_manifest(dest_dir, &encoded)
        .map_err(|error| publication_failed(BackupStage::Verifying, dest_dir, error))?;
    manifest::write_backup_meta(dest_dir)
        .map_err(|error| publication_failed(BackupStage::Verifying, dest_dir, error))?;
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
