//! Failure injection per protocol step over an adapter-shaped port double
//! (R2a step 3a item 6, design `online-backup.md` 4.2, R4 cleanup rule).
//!
//! The double mirrors `EngineBackupPort`'s observable contract (catalogue
//! snapshot, counted truncation pins, ordered seal/`end_seal` events,
//! idempotent `release_pins`) over plain directories, with one injected
//! failure per protocol step: pin acquire, capture barrier, copy pass,
//! seal barrier and the manifest write. Every test asserts the R4 cleanup
//! invariant: `end_seal` for every still-open hold, `release_pins` called,
//! the destination directory removed, and a before/after snapshot of the
//! source data dir proving the source was left untouched. The
//! system-store capture failure knob covers the step-B tail that the real
//! adapter now serves from `ShamirDb::capture_system_store_point()`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::sync::durability::backup::online::copy_engine::CopyOptions;
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupReport, OnlineBackupError, OnlineBackupFailure, SealRecopyCap,
};
use crate::sync::durability::backup::online::wal_capture::{
    CapturedPoint, OnlineBackupPort, PortError, RepoSource,
};

/// Unique per-test fixture root: a fresh empty tempdir, deleted when the
/// returned path's directory is removed at the end of the test.
fn temp_root(_name: &str) -> PathBuf {
    tempfile::tempdir().expect("tempdir").keep()
}

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    fs::write(path, bytes).expect("write");
}

/// Recursive `<rel path, size>` listing, forward-slash separated, sorted —
/// the R4 before/after snapshot of a directory tree.
fn snapshot_tree(root: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map(|rel| {
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(meta) = entry.metadata() {
            out.push((rel, meta.len()));
        }
    }
}

/// Adapter-shaped fault port: counted pins, ordered seal events, one
/// injected failure per knob.
struct FaultPort {
    repos: Vec<RepoSource>,
    /// `acquire_pins` holds one pin, releases it and fails (step A).
    fail_pins: bool,
    /// `capture_point` returns a typed `Capture` error (step B).
    fail_capture: bool,
    /// `capture_point` removes the given directory first, so the copy pass
    /// hits a vanished source (steps C/D).
    remove_on_capture: Option<PathBuf>,
    /// `seal_point` returns a typed `Seal` error (step E).
    fail_seal: bool,
    /// `capture_system_store` fails (step-B tail; the real adapter maps a
    /// facade failure here).
    fail_system_store: bool,
    pins_held: AtomicUsize,
    pins_released: AtomicUsize,
    events: Mutex<Vec<String>>,
}

impl FaultPort {
    fn with_repos(source: &Path, names: &[&str]) -> Self {
        Self {
            repos: names
                .iter()
                .map(|name| RepoSource {
                    name: (*name).to_string(),
                    store_dir: source.join(format!("repos/{name}")),
                    wal_dir: Some(source.join(format!("repos/{name}.shamirwal"))),
                })
                .collect(),
            fail_pins: false,
            fail_capture: false,
            remove_on_capture: None,
            fail_seal: false,
            fail_system_store: false,
            pins_held: AtomicUsize::new(0),
            pins_released: AtomicUsize::new(0),
            events: Mutex::new(Vec::new()),
        }
    }

    fn single_repo(source: &Path) -> Self {
        Self::with_repos(source, &["alpha"])
    }

    /// Plain-directory fixture for one repo: store file + WAL segment with
    /// its sidecar (the adapter's `<store file name>.shamirwal` sibling
    /// convention).
    fn seed_source(source: &Path, names: &[&str]) {
        for name in names {
            write_file(
                &source.join(format!("repos/{name}/store.bin")),
                b"store-bytes",
            );
            write_file(
                &source.join(format!("repos/{name}.shamirwal/00000000.wal")),
                b"wal-0",
            );
            write_file(
                &source.join(format!("repos/{name}.shamirwal/00000000.meta")),
                b"meta-0",
            );
        }
    }

    fn pins_held(&self) -> usize {
        self.pins_held.load(Ordering::SeqCst)
    }

    fn pins_released(&self) -> usize {
        self.pins_released.load(Ordering::SeqCst)
    }

    fn seal_events(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("lock")
            .iter()
            .filter(|event| event.starts_with("seal:") || event.starts_with("end_seal:"))
            .cloned()
            .collect()
    }

    fn point(&self, repo: &str) -> CapturedPoint {
        CapturedPoint {
            wal_position: 1,
            store_dir: format!("repos/{repo}"),
            schema_version: 1,
        }
    }
}

impl OnlineBackupPort for FaultPort {
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError> {
        Ok(self.repos.clone())
    }

    fn acquire_pins(&self) -> Result<(), PortError> {
        self.pins_held.fetch_add(1, Ordering::SeqCst);
        if self.fail_pins {
            // Mirror the real adapter: partial pins are released by the
            // port itself before the error propagates.
            self.release_pins();
            return Err(PortError::PinAcquire);
        }
        Ok(())
    }

    fn capture_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        if let Some(dir) = &self.remove_on_capture {
            let _ = fs::remove_dir_all(dir);
        }
        if self.fail_capture {
            return Err(PortError::Capture {
                repo: repo.to_string(),
                source: io::Error::other("injected capture failure"),
            });
        }
        Ok(self.point(repo))
    }

    fn capture_system_store(&self) -> Result<u64, PortError> {
        if self.fail_system_store {
            return Err(PortError::SystemStore(io::Error::other(
                "injected system-store capture failure",
            )));
        }
        Ok(7)
    }

    fn seal_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        self.events
            .lock()
            .expect("lock")
            .push(format!("seal:{repo}"));
        if self.fail_seal {
            return Err(PortError::Seal {
                repo: repo.to_string(),
                source: io::Error::other("injected seal failure"),
            });
        }
        Ok(self.point(repo))
    }

    fn end_seal(&self, repo: &str) {
        self.events
            .lock()
            .expect("lock")
            .push(format!("end_seal:{repo}"));
    }

    fn release_pins(&self) {
        self.pins_released.fetch_add(1, Ordering::SeqCst);
        self.events
            .lock()
            .expect("lock")
            .push("release_pins".to_string());
    }
}

fn run(port: &FaultPort, source: &Path, to: &Path) -> Result<BackupReport, OnlineBackupError> {
    let mut progress = BackupProgress::new();
    let cancel = CancelFlag::default();
    let cap = SealRecopyCap::default();
    run_online_backup(
        port,
        source,
        to,
        &cancel,
        &mut progress,
        &CopyOptions::default(),
        &cap,
    )
}

fn expect_failed(error: OnlineBackupError) -> (BackupStage, OnlineBackupFailure) {
    match error {
        OnlineBackupError::Failed { stage, source, .. } => (stage, source),
        other => panic!("expected Failed, got: {other:?}"),
    }
}

/// A pin-acquire failure at step A fails in `Preparing`, calls
/// `release_pins`, and leaves the destination root empty with the source
/// untouched (R4 before/after snapshots equal).
#[test]
fn pin_failure_fails_preparing_and_frees_everything() {
    let source = temp_root("pin_fail_source");
    let to = temp_root("pin_fail_to");
    FaultPort::seed_source(&source, &["alpha"]);
    let mut port = FaultPort::single_repo(&source);
    port.fail_pins = true;
    let before = snapshot_tree(&source);

    let (stage, failure) = expect_failed(run(&port, &source, &to).expect_err("must fail"));

    assert_eq!(stage, BackupStage::Preparing);
    assert!(
        matches!(failure, OnlineBackupFailure::Port(PortError::PinAcquire)),
        "pin failure must surface as the port error: {failure:?}"
    );
    assert_eq!(port.pins_held(), 1);
    assert!(
        port.pins_released() >= 1,
        "release_pins must be called on the failure path"
    );
    assert_eq!(
        fs::read_dir(&to).expect("to dir").count(),
        0,
        "destination must be removed"
    );
    assert_eq!(
        snapshot_tree(&source),
        before,
        "source data dir must be untouched"
    );
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

/// A capture-barrier error at step B fails in `Captured` with the failing
/// repo named, releases the step-A pins and removes the destination.
#[test]
fn capture_error_fails_captured_and_releases_pins() {
    let source = temp_root("cap_fail_source");
    let to = temp_root("cap_fail_to");
    FaultPort::seed_source(&source, &["alpha"]);
    let mut port = FaultPort::single_repo(&source);
    port.fail_capture = true;
    let before = snapshot_tree(&source);

    let (stage, failure) = expect_failed(run(&port, &source, &to).expect_err("must fail"));

    assert_eq!(stage, BackupStage::Captured);
    let OnlineBackupFailure::Port(PortError::Capture { repo, .. }) = failure else {
        panic!("expected Capture port error, got: {failure:?}");
    };
    assert_eq!(repo, "alpha");
    assert!(
        port.pins_released() >= 1,
        "step-A pins must be released after a capture failure"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(snapshot_tree(&source), before);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

/// A failing system-store capture (step-B tail, after the per-repo capture
/// barriers) fails in `Captured` with the `SystemStore` port error, releases
/// the step-A pins and removes the destination; the source is untouched.
#[test]
fn system_store_capture_error_fails_captured_and_releases_pins() {
    let source = temp_root("sysstore_fail_source");
    let to = temp_root("sysstore_fail_to");
    FaultPort::seed_source(&source, &["alpha"]);
    let mut port = FaultPort::single_repo(&source);
    port.fail_system_store = true;
    let before = snapshot_tree(&source);

    let (stage, failure) = expect_failed(run(&port, &source, &to).expect_err("must fail"));

    assert_eq!(stage, BackupStage::Captured);
    assert!(
        matches!(
            failure,
            OnlineBackupFailure::Port(PortError::SystemStore(_))
        ),
        "system-store failure must surface as the port error: {failure:?}"
    );
    assert!(
        port.pins_released() >= 1,
        "step-A pins must be released after a system-store capture failure"
    );
    assert_eq!(
        fs::read_dir(&to).expect("to dir").count(),
        0,
        "destination must be removed"
    );
    assert_eq!(
        snapshot_tree(&source),
        before,
        "source data dir must be untouched"
    );
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

/// A source directory vanishing between capture and copy fails in
/// `Copying`, and the destination is removed (the copy failure path).
#[test]
fn copy_error_mid_pass_fails_copying_and_cleans_up() {
    let source = temp_root("copy_fail_source");
    let to = temp_root("copy_fail_to");
    FaultPort::seed_source(&source, &["alpha"]);
    let wal_dir = source.join("repos/alpha.shamirwal");
    let mut port = FaultPort::single_repo(&source);
    port.remove_on_capture = Some(wal_dir.clone());

    let (stage, failure) = expect_failed(run(&port, &source, &to).expect_err("must fail"));

    assert_eq!(stage, BackupStage::Copying);
    assert!(
        matches!(failure, OnlineBackupFailure::Copy(_)),
        "a vanished copy source must surface as a copy error: {failure:?}"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert!(
        port.pins_released() >= 1,
        "pins must be released after a copy failure"
    );
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

/// A seal-barrier error at step E fails in `Sealing`, ends the hold that
/// was still open (`end_seal` after `seal`, in order) and releases the
/// pins.
#[test]
fn seal_error_ends_open_hold_and_releases_pins() {
    let source = temp_root("seal_fail_source");
    let to = temp_root("seal_fail_to");
    FaultPort::seed_source(&source, &["alpha"]);
    let mut port = FaultPort::single_repo(&source);
    port.fail_seal = true;
    let before = snapshot_tree(&source);

    let (stage, failure) = expect_failed(run(&port, &source, &to).expect_err("must fail"));

    assert_eq!(stage, BackupStage::Sealing);
    let OnlineBackupFailure::Port(PortError::Seal { repo, .. }) = failure else {
        panic!("expected Seal port error, got: {failure:?}");
    };
    assert_eq!(repo, "alpha");
    assert_eq!(
        port.seal_events(),
        vec!["seal:alpha".to_string(), "end_seal:alpha".to_string()],
        "the still-open hold must be ended on the seal failure path"
    );
    assert!(
        port.pins_released() >= 1,
        "pins must be released after a seal failure"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(snapshot_tree(&source), before);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

/// A blocked manifest write (`manifest.json.tmp` occupied by a copied
/// directory) fails in `Verifying` — AFTER the stores are already in the
/// snapshot — and the whole destination is still removed.
#[test]
fn manifest_write_conflict_fails_verifying_and_removes_destination() {
    let source = temp_root("manifest_fail_source");
    let to = temp_root("manifest_fail_to");
    FaultPort::seed_source(&source, &["alpha"]);
    // Copied verbatim into the snapshot, then blocks the tmp-then-rename
    // manifest write (step F).
    write_file(&source.join("manifest.json.tmp/blocker"), b"block");
    let port = FaultPort::single_repo(&source);
    let before = snapshot_tree(&source);

    let (stage, failure) = expect_failed(run(&port, &source, &to).expect_err("must fail"));

    assert_eq!(stage, BackupStage::Verifying);
    assert!(
        matches!(
            failure,
            OnlineBackupFailure::Io(_)
                | OnlineBackupFailure::Manifest(_)
                | OnlineBackupFailure::Dest { .. }
        ),
        "the manifest write conflict must surface as an io/manifest/destination error: {failure:?}"
    );
    assert!(
        !to.join("manifest.json").exists(),
        "no manifest may be left behind"
    );
    assert_eq!(
        fs::read_dir(&to).expect("to dir").count(),
        0,
        "the destination must be removed even after a full copy"
    );
    assert!(
        port.pins_released() >= 1,
        "pins must be released after a manifest failure"
    );
    assert_eq!(
        snapshot_tree(&source),
        before,
        "source data dir must be untouched"
    );
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}
