//! `OnlineBackupPort` implementation over the engine facade (R2a step 3a).
//!
//! Design: `docs/dev-artifacts/design/online-backup.md` §4.2 (steps A–G),
//! Q8 (in-memory repos), Q9 (catalogue snapshot). The adapter drives the
//! synchronous protocol from a blocking thread and bridges to the async
//! `RepoInstance` facade (`capture_point` / `seal_point` /
//! `acquire_truncation_pin`, `crates/engine/shamir-engine/src/repo/instance/
//! repo_instance/lifecycle/online_backup.rs`) through the server's runtime
//! handle (`Handle::block_on` on a blocking thread — never on a runtime
//! worker; large futures are `Box::pin`ed in the bridge helper per the
//! dispatch-depth rule).
//!
//! Facade gaps (reported; NOT worked around by reaching into internals):
//! * `CapturedPoint::schema_version` has no facade accessor; the manifest
//!   v2 field is reported as `0` (informational metadata, not a
//!   consistency point).
//!
//! The system-store consistency point comes from
//! `ShamirDb::capture_system_store_point()` (design Q4 / protocol step B):
//! it resolves the SYSTEM repository and runs its `capture_point()` —
//! flush under the writer-drain barrier, reporting the highest
//! fully-captured SYSTEM-repo WAL position, monotonic across catalogue
//! writes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use shamir_collections::TFxMap;
use shamir_db::engine::repo::repo_instance::{OnlineBackupError, SealHold, TruncationPinGuard};
use shamir_db::engine::repo::RepoInstance;
use shamir_db::ShamirDb;
use shamir_types::types::value::QueryValue;
use tokio::runtime::Handle;

use crate::sync::durability::backup::online::progress::{BackupStage, CancelFlag};
use crate::sync::durability::backup::online::wal_capture::{
    CapturedPoint, OnlineBackupPort, PortError, RepoSource,
};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

/// Stage hints written by the adapter at port-call boundaries. The protocol
/// has no progress callback, so the registry derives the live stage from
/// these hints (design §4.7 progress: stage-level fidelity; per-repo
/// hashed-byte counters become visible only after the run updates them
/// through `BackupProgress`).
pub mod stage_hint {
    pub const PREPARING: u8 = 0;
    pub const COPYING: u8 = 1;
    pub const SEALING: u8 = 2;
    pub const VERIFYING: u8 = 3;
}

/// State the adapter shares with the registry while a run executes.
#[derive(Debug, Default)]
pub struct PortRunState {
    /// Stage hint (see [`stage_hint`]).
    stage: AtomicU8,
    /// Repos excluded from the captured set, with the reason (Q8/Q9).
    excluded: Mutex<Vec<String>>,
    /// Number of file-backed repos in the step-A catalogue snapshot.
    snapshot_count: Mutex<usize>,
}

impl PortRunState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Repos excluded from the captured set (in-memory, Q8; no live
    /// instance — an inert orphan dir still copied by the residual pass).
    pub fn excluded_repos(&self) -> Vec<String> {
        self.excluded.lock().clone()
    }

    /// Repos in the step-A catalogue snapshot.
    pub fn snapshot_count(&self) -> usize {
        *self.snapshot_count.lock()
    }

    /// Stage hint mapped to a protocol stage.
    pub fn stage(&self) -> BackupStage {
        match self.stage.load(Ordering::Acquire) {
            stage_hint::COPYING => BackupStage::Copying,
            stage_hint::SEALING => BackupStage::Sealing,
            stage_hint::VERIFYING => BackupStage::Verifying,
            _ => BackupStage::Preparing,
        }
    }

    fn set_stage(&self, stage: u8) {
        self.stage.store(stage, Ordering::Release);
    }
}

/// The port over the engine facade. One instance per run: the catalogue is
/// snapshotted on the first `catalogue()` call (step A) and never re-read
/// (Q9).
pub struct EngineBackupPort {
    db: Arc<ShamirDb>,
    /// The server data dir (the protocol's `source_root`).
    data_dir: PathBuf,
    /// The server runtime handle; async facade calls are bridged through it.
    runtime: Handle,
    run: Arc<PortRunState>,
    /// `"<db>/<repo>"` -> `(db, repo, store_dir)` for the captured set.
    captured: Mutex<TFxMap<String, (String, String, PathBuf)>>,
    /// Step-A truncation pins, held until `release_pins` (drop; idempotent).
    pins: Mutex<Vec<TruncationPinGuard>>,
    /// Seal holds per repo, kept until `end_seal` (design "Step 1d
    /// decisions" (a)); removing the entry drops the hold (idempotent).
    seals: Mutex<TFxMap<String, SealHold>>,
}

impl EngineBackupPort {
    /// Build the port for one run. `data_dir` must be the same root the
    /// protocol copies (`source_root`).
    pub fn new(
        db: Arc<ShamirDb>,
        data_dir: PathBuf,
        runtime: Handle,
        run: Arc<PortRunState>,
    ) -> Self {
        Self {
            db,
            data_dir,
            runtime,
            run,
            captured: Mutex::new(TFxMap::default()),
            pins: Mutex::new(Vec::new()),
            seals: Mutex::new(TFxMap::default()),
        }
    }

    /// Bridge an async facade call onto the runtime handle from the
    /// blocking thread. `Box::pin`: large engine futures at the call site
    /// (dispatch-depth rule).
    fn bridge<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(Box::pin(future))
    }

    /// The engine seal barrier under cancellation and the repo's absolute
    /// seal budget deadline. The barrier future runs to its NATURAL
    /// completion — it is never dropped, `select`ed against or wrapped in
    /// `timeout_at` from outside (R2a follow-up 3): the controlled facade
    /// [`RepoInstance::seal_point_controlled`] consults the caller's
    /// cancellation closure and the deadline ONLY at its own named step
    /// boundaries, strictly between mutating awaits (gate-close RMW,
    /// zero-lease wait, drain, pause acquisition, recheck, flush). A fired
    /// checkpoint returns the typed
    /// [`OnlineBackupError::Cancelled`] / [`OnlineBackupError::DeadlineExceeded`]
    /// and the engine's RAII owners fail closed on that ordinary error
    /// return — pause released first, gate reopened last, no snapshot.
    /// In-flight blocking I/O is not preempted: a barrier step blocked on
    /// a straggler syscall runs to completion before the next checkpoint
    /// is observed, so a straggler can extend the closed window —
    /// fail-closed either way (the protocol removes the destination).
    fn seal_bridge(
        &self,
        repo: &str,
        instance: RepoInstance,
        deadline: Option<Instant>,
        cancel: &CancelFlag,
    ) -> Result<SealHold, PortError> {
        // The closure is consulted by the engine at each step boundary;
        // the flag is a cheap atomic/token check (no locking, no await).
        let cancel = cancel.clone();
        self.bridge(instance.seal_point_controlled(deadline, move || cancel.is_cancelled()))
            .map_err(|error| PortError::Seal {
                repo: repo.to_string(),
                source: Self::barrier_error(repo, error),
            })
    }

    /// Resolve the live repo instance for a captured `"db/repo"` key.
    fn resolve(&self, key: &str) -> Option<RepoInstance> {
        let (db_name, repo_name, _store_dir) = self.captured.lock().get(key)?.clone();
        self.db.get_db(&db_name)?.get_repo(&repo_name)
    }

    /// Map a typed barrier failure into a `PortError`, preserving the repo
    /// name and the failure text (facade errors carry no `io::Error`).
    fn barrier_error(repo: &str, error: OnlineBackupError) -> std::io::Error {
        io::Error::other(format!("repo {repo}: {error}"))
    }

    /// The store dir's path relative to the data dir, forward-slash
    /// separated (mirrors the protocol's manifest paths).
    fn store_dir_rel(&self, store_dir: &Path) -> String {
        store_dir
            .strip_prefix(&self.data_dir)
            .map(rel_slash)
            .unwrap_or_else(|_| store_dir.to_string_lossy().into_owned())
    }
}

/// Copy a Fjall directory for validation without exposing the backup tree to recovery writes.
fn copy_validation_tree(source: &Path, destination: &Path, depth: usize) -> io::Result<()> {
    const MAX_DEPTH: usize = 64;
    if depth > MAX_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "server_meta tree is too deep",
        ));
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported server_meta symlink",
            ));
        } else if metadata.is_dir() {
            copy_validation_tree(&source_path, &destination_path, depth + 1)?;
        } else if metadata.is_file() {
            fs::copy(source_path, destination_path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported server_meta entry",
            ));
        }
    }
    Ok(())
}

struct ValidationTempDir(PathBuf);

impl ValidationTempDir {
    fn new() -> io::Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "shamir-backup-validation-{}-{nonce}-{id}",
                std::process::id()
            ));
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Ok(_) => continue,
                Err(error) => return Err(error),
            }
            match shamir_private_dir::create_private_dir(&path) {
                Ok(()) => {
                    if fs::read_dir(&path)?.next().is_some() {
                        continue;
                    }
                    return Ok(Self(path));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate validation directory",
        ))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ValidationTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn is_audit_archive_name(name: &str) -> bool {
    name.strip_prefix("audit.log.").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn validate_copied_data_dir(copied_data_dir: &Path) -> Result<(), PortError> {
    let source = copied_data_dir.join("server_meta");
    let source_metadata = fs::symlink_metadata(&source).map_err(PortError::SystemStore)?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(PortError::SystemStore(io::Error::new(
            io::ErrorKind::InvalidData,
            "copied server_meta is not a regular directory",
        )));
    }
    let temp = ValidationTempDir::new().map_err(PortError::SystemStore)?;
    let validation_copy = temp.path().join("server_meta");
    copy_validation_tree(&source, &validation_copy, 0).map_err(PortError::SystemStore)?;
    for entry in fs::read_dir(copied_data_dir).map_err(PortError::SystemStore)? {
        let entry = entry.map_err(PortError::SystemStore)?;
        let name = entry.file_name();
        let text = name.to_string_lossy();
        if text != "audit.log" && !is_audit_archive_name(&text) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(PortError::SystemStore)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(PortError::SystemStore(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported audit validation entry",
            )));
        }
        fs::copy(entry.path(), temp.path().join(name)).map_err(PortError::SystemStore)?;
    }
    let store = crate::server_meta::open_existing(&validation_copy).map_err(|error| {
        PortError::SystemStore(io::Error::other(format!(
            "copied server_meta cannot open: {error}"
        )))
    })?;
    let key = store.audit_chain_key().map_err(|error| {
        PortError::SystemStore(io::Error::other(format!(
            "copied audit key cannot be read: {error}"
        )))
    })?;
    let (appender, _) = store
        .open_audit_recovered_strict(temp.path(), &key)
        .map_err(|error| {
            PortError::SystemStore(io::Error::other(format!(
                "copied audit chain does not validate: {error}"
            )))
        })?;
    drop(appender);
    drop(store);
    Ok(())
}

/// Forward-slash relative path string.
fn rel_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

impl OnlineBackupPort for EngineBackupPort {
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError> {
        let records = self
            .bridge(self.db.system_store().load_repositories())
            .map_err(|error| {
                PortError::SystemStore(io::Error::other(format!("catalogue load failed: {error}")))
            })?;
        let mut sources = Vec::new();
        let mut captured = self.captured.lock();
        captured.clear();
        for record in &records {
            let QueryValue::Map(fields) = record else {
                continue;
            };
            let Some(db_name) = fields.get("db_name").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(repo_name) = fields.get("repo_name").and_then(|v| v.as_str()) else {
                continue;
            };
            let _engine = fields.get("engine").and_then(|v| v.as_str());
            let path = fields.get("path").and_then(|v| v.as_str());
            let qualified = format!("{db_name}/{repo_name}");
            // Q8: in-memory repos have no files; reported, never copied.
            let Some(store_dir) = path.map(PathBuf::from) else {
                self.run
                    .excluded
                    .lock()
                    .push(format!("{qualified} (in-memory, not copied)"));
                continue;
            };
            // A catalogue row without a live instance is a dropped repo's
            // inert orphan dir (Q9): still copied by the protocol's residual
            // pass, but there is nothing to pin, capture or seal.
            let Some(_instance) = self
                .db
                .get_db(db_name)
                .and_then(|db| db.get_repo(repo_name))
            else {
                self.run
                    .excluded
                    .lock()
                    .push(format!("{qualified} (dropped; dir copied as inert orphan)"));
                continue;
            };
            // WAL dir convention (design 1.3 WAL / durability_access): a
            // sibling of the store dir, `<store file name>.shamirwal`.
            let wal_dir = store_dir.file_name().map(|name| {
                let mut wal = name.to_os_string();
                wal.push(".shamirwal");
                store_dir.with_file_name(wal)
            });
            captured.insert(
                qualified.clone(),
                (
                    db_name.to_string(),
                    repo_name.to_string(),
                    store_dir.clone(),
                ),
            );
            sources.push(RepoSource {
                name: qualified,
                store_dir,
                wal_dir,
            });
        }
        *self.run.snapshot_count.lock() = sources.len();
        Ok(sources)
    }

    fn acquire_pins(&self) -> Result<(), PortError> {
        let keys: Vec<String> = self.captured.lock().keys().cloned().collect();
        let mut pins = self.pins.lock();
        for key in &keys {
            let Some(repo) = self.resolve(key) else {
                // Clear under the already-held pins lock: calling
                // `release_pins` here would re-lock the same non-reentrant
                // mutex (self-deadlock).
                pins.clear();
                drop(pins);
                return Err(PortError::PinAcquire);
            };
            // Floor 0: retain the whole WAL history for the copy's duration
            // (the seal point is only known at step E; design 4.2 A pins
            // everything from step A).
            match self.bridge(repo.acquire_truncation_pin(0)) {
                Ok(guard) => pins.push(guard),
                Err(_) => {
                    drop(pins);
                    self.release_pins();
                    return Err(PortError::PinAcquire);
                }
            }
        }
        Ok(())
    }

    fn capture_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        let Some(instance) = self.resolve(repo) else {
            return Err(PortError::Capture {
                repo: repo.to_string(),
                source: io::Error::other("repo vanished from the captured set"),
            });
        };
        let position =
            self.bridge(instance.capture_point())
                .map_err(|error| PortError::Capture {
                    repo: repo.to_string(),
                    source: Self::barrier_error(repo, error),
                })?;
        Ok(CapturedPoint {
            wal_position: position,
            store_dir: self.store_dir_for(repo),
            schema_version: 0,
        })
    }

    fn capture_system_store(&self) -> Result<u64, PortError> {
        // The system store IS a `RepoInstance` (SYSTEM repo): the facade
        // runs its capture barrier (writer-drain quiesce bounded inside the
        // call) and reports the highest fully-captured SYSTEM-repo WAL
        // position — monotonic across catalogue writes (design Q4, step B;
        // manifest `consistency.system_store_version` source).
        self.bridge(self.db.capture_system_store_point())
            .map_err(|error| {
                PortError::SystemStore(io::Error::other(format!(
                    "system store capture point failed: {error}"
                )))
            })
    }

    fn seal_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        // Direct variant without deadline/cancel: kept for existing tests
        // (the protocol's step E calls
        // [`OnlineBackupPort::seal_point_before`]).
        self.seal_point_before(repo, None, &CancelFlag::default())
    }

    /// Step E, deadline-aware (R2a-4): the SAME absolute monotonic
    /// `deadline` the caller computed and the caller's cancellation flag
    /// are handed to the engine's controlled seal barrier
    /// ([`RepoInstance::seal_point_controlled`]), which observes them only
    /// at its own step boundaries — the barrier future itself is NEVER
    /// dropped mid-await (see [`Self::seal_bridge`]). A fired checkpoint
    /// refuses the barrier through the engine's ordinary typed error path
    /// (fail-closed: pause released, gate reopened, no snapshot) and the
    /// protocol removes the destination. In-flight blocking I/O is not
    /// preempted: a step blocked on a straggler syscall completes before
    /// the next checkpoint fires, so a straggler can extend the closed
    /// window — the run still fails closed. Not covering process teardown
    /// mid-syscall.
    fn seal_point_before(
        &self,
        repo: &str,
        deadline: Option<Instant>,
        cancel: &CancelFlag,
    ) -> Result<CapturedPoint, PortError> {
        let Some(instance) = self.resolve(repo) else {
            return Err(PortError::Seal {
                repo: repo.to_string(),
                source: io::Error::other("repo vanished from the captured set"),
            });
        };
        self.run.set_stage(stage_hint::SEALING);
        let hold = self.seal_bridge(repo, instance, deadline, cancel)?;
        let point = CapturedPoint {
            wal_position: hold.position(),
            store_dir: self.store_dir_for(repo),
            schema_version: 0,
        };
        self.seals.lock().insert(repo.to_string(), hold);
        Ok(point)
    }

    /// Called at the C start, before the first WAL pass: writers are
    /// visibly RUNNING again, so the live status publishes the real phase
    /// `Copying` and e2e tests can prove the copy overlaps the
    /// writer-running phase.
    fn begin_copy(&self) {
        self.run.set_stage(stage_hint::COPYING);
    }

    /// Called at the real F start: the live status moves from `Sealing`
    /// to `Verifying` once the seal loop over the repos has finished, so
    /// end_seal no longer publishes a misleading Verifying stage early.
    fn begin_verify(&self) {
        self.run.set_stage(stage_hint::VERIFYING);
    }

    fn validate_copied_data_dir(&self, copied_data_dir: &Path) -> Result<(), PortError> {
        validate_copied_data_dir(copied_data_dir)
    }

    fn end_seal(&self, repo: &str) {
        // The stage hint stays SEALING here: the protocol's seal loop
        // continues over the remaining repos, and VERIFYING is published
        // only later by `begin_verify` at the real F start. The hold is
        // extracted under the lock and dropped OUTSIDE it: its `Drop` reopens the
        // commit gate (a measured, potentially slow synchronous reopen)
        // and must not hold the seal-map mutex while doing so.
        let hold = self.seals.lock().remove(repo);
        drop(hold);
    }

    fn release_pins(&self) {
        self.pins.lock().clear();
    }
}

impl EngineBackupPort {
    /// Test-only narrow access: number of step-A pins currently held on
    /// this adapter (the production surface stays closed; regression
    /// evidence that a failed `acquire_pins` released everything).
    #[cfg(test)]
    pub(crate) fn test_held_pins(&self) -> usize {
        self.pins.lock().len()
    }

    /// The captured repo's store dir path relative to the data dir, from
    /// the step-A snapshot (the instance exposes no path accessor).
    fn store_dir_for(&self, repo: &str) -> String {
        match self.captured.lock().get(repo) {
            Some((_db, _name, store_dir)) => self.store_dir_rel(store_dir),
            None => String::new(),
        }
    }
}
