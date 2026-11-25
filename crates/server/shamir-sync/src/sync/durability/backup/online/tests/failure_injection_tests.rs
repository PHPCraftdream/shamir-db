//! Failure-injection tests of the online backup protocol (design
//! `online-backup.md` 6.3, requirement R4): a fault at every protocol
//! stage A–G and a cancellation at every injectable stage boundary must
//! leave the source directory byte-identical, release the truncation
//! pins (observable as the pin counter returning to zero), remove every
//! partial output byte, and report the actual failed stage and error
//! through the progress snapshot and the error's source chain.
//!
//! Faults are injected through the port hooks only (the existing seam
//! style of `protocol_tests.rs`); filesystem faults are fixture acts (a
//! hook mutates a path mid-protocol), never protocol restoration.
//!
//! The pin-failure case models a PARTIAL acquisition (the second of two
//! repos failing AFTER the first pin was acquired) and establishes the
//! pre-release held count through the port's `released_from` record —
//! it REPLACES the previous single-repo no-pin-acquired failure case
//! (the server-side `pin_failure_fails_preparing_and_frees_everything`
//! and capture/seal/system/manifest fault cases A/B/E/F in
//! `crates/server/shamir-server/.../backup_online/port_fault_injection_tests.rs`
//! overlap with the remaining single-repo cases here and were left
//! untouched). The deleted-source FAILURE case added in
//! `copy_seam_tests.rs` (deletion after the file's OWN copy leaves a
//! stale destination file, refused at F) is distinct from the existing
//! server-side whole-WAL-dir-removal test, which deletes before any
//! listing.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use super::protocol_tests::TestPort;
use crate::sync::durability::backup::online::copy_engine::{CopyError, CopyOptions};
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupReport, OnlineBackupError, OnlineBackupFailure, SealRecopyCap,
};
use crate::sync::durability::backup::online::wal_capture::{
    CapturedPoint, OnlineBackupPort, PortError, RepoSource,
};

pub(crate) fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    fs::write(path, bytes).expect("write");
}

/// Byte-exact content snapshot of every regular file under `root`,
/// forward-slash relative paths (R4: the source must be identical before
/// and after a failed or cancelled run).
pub(crate) fn snapshot_dir(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("read source dir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                stack.push(entry);
            } else {
                let rel = entry
                    .strip_prefix(root)
                    .expect("under root")
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(rel, fs::read(&entry).expect("read source file"));
            }
        }
    }
    out
}

pub(crate) fn assert_source_untouched(before: &BTreeMap<String, Vec<u8>>, source: &Path) {
    let after = snapshot_dir(source);
    assert_eq!(
        &after, before,
        "R4: the source directory must be byte-identical after a failed run"
    );
}

/// The protocol's destination root holds exactly one timestamped child
/// (the snapshot directory) once step A ran; located on demand — the
/// timestamp itself is irrelevant, the root is unique per fixture.
fn snapshot_dir_of(to: &Path) -> PathBuf {
    let mut children: Vec<PathBuf> = fs::read_dir(to)
        .expect("read destination root")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    assert_eq!(
        children.len(),
        1,
        "the destination root must hold exactly the snapshot directory"
    );
    children.pop().expect("non-empty")
}

/// Hook fired mid-protocol; receives the fixture's destination ROOT (the
/// snapshot directory is resolved from it on demand).
type RootHook = Box<dyn Fn(&Path) + Send>;

/// Faults injectable at each port call.
#[derive(Default)]
struct Faults {
    catalogue_error: bool,
    acquire_pins_error: bool,
    /// Partial-acquisition model: the first of two pins is acquired,
    /// then the acquisition fails — a pin is genuinely held when the
    /// failure propagates.
    acquire_pins_partial_error: bool,
    capture_error: bool,
    capture_system_error: bool,
    seal_error: bool,
}

pub(crate) struct InjectPort {
    repos: Vec<RepoSource>,
    faults: Faults,
    /// Pins currently held. `release_pins` resets it to zero — the
    /// observable state the real port's pin holders would see — and
    /// counts release calls for the exactly-once assertion.
    pub(crate) pins_held: Mutex<usize>,
    pub(crate) pins_released: AtomicUsize,
    /// The held count observed by `release_pins` immediately BEFORE the
    /// reset — establishes how many pins were still held at cleanup
    /// time (the partial-acquisition case's pre-release evidence).
    pub(crate) released_from: AtomicUsize,
    on_pins: Mutex<Option<RootHook>>,
    on_capture: Mutex<Option<RootHook>>,
    on_capture_system: Mutex<Option<RootHook>>,
    on_seal: Mutex<Option<RootHook>>,
    on_end_seal: Mutex<Option<RootHook>>,
}

impl InjectPort {
    pub(crate) fn single_repo(source: &Path) -> Self {
        Self::with_repos(source, &["alpha"])
    }

    pub(crate) fn two_repos(source: &Path) -> Self {
        Self::with_repos(source, &["alpha", "beta"])
    }

    pub(crate) fn with_repos(source: &Path, names: &[&str]) -> Self {
        Self {
            repos: names
                .iter()
                .map(|name| RepoSource {
                    name: (*name).to_string(),
                    store_dir: source.join(format!("repos/{name}")),
                    wal_dir: Some(source.join(format!("repos/{name}.shamirwal"))),
                })
                .collect(),
            faults: Faults::default(),
            pins_held: Mutex::new(0),
            pins_released: AtomicUsize::new(0),
            released_from: AtomicUsize::new(0),
            on_pins: Mutex::new(None),
            on_capture: Mutex::new(None),
            on_capture_system: Mutex::new(None),
            on_seal: Mutex::new(None),
            on_end_seal: Mutex::new(None),
        }
    }

    fn with_faults(mut self, faults: Faults) -> Self {
        self.faults = faults;
        self
    }

    pub(crate) fn pins_currently_held(&self) -> usize {
        *self.pins_held.lock().expect("lock")
    }

    fn with_pins_hook(self, hook: impl Fn(&Path) + Send + 'static) -> Self {
        *self.on_pins.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    fn with_capture_hook(self, hook: impl Fn(&Path) + Send + 'static) -> Self {
        *self.on_capture.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    fn with_seal_hook(self, hook: impl Fn(&Path) + Send + 'static) -> Self {
        *self.on_seal.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    pub(crate) fn with_end_seal_hook(self, hook: impl Fn(&Path) + Send + 'static) -> Self {
        *self.on_end_seal.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    pub(crate) fn source_with_data(source: &Path) {
        for name in ["alpha", "beta"] {
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
}

impl OnlineBackupPort for InjectPort {
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError> {
        if self.faults.catalogue_error {
            return Err(PortError::SystemStore(std::io::Error::other(
                "catalogue read failed",
            )));
        }
        Ok(self.repos.clone())
    }

    fn acquire_pins(&self) -> Result<(), PortError> {
        if let Some(hook) = self.on_pins.lock().expect("lock").as_ref() {
            hook(Path::new(""));
        }
        if self.faults.acquire_pins_error || self.faults.acquire_pins_partial_error {
            if self.faults.acquire_pins_partial_error {
                // Second of two repos failing AFTER the first pin was
                // acquired: one pin is genuinely held on the error path.
                *self.pins_held.lock().expect("lock") += 1;
            }
            return Err(PortError::PinAcquire);
        }
        *self.pins_held.lock().expect("lock") += 1;
        Ok(())
    }

    fn capture_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        if let Some(hook) = self.on_capture.lock().expect("lock").as_ref() {
            hook(Path::new(""));
        }
        if self.faults.capture_error {
            return Err(PortError::Capture {
                repo: repo.to_string(),
                source: std::io::Error::other("flush failed"),
            });
        }
        Ok(CapturedPoint {
            wal_position: 1,
            store_dir: format!("repos/{repo}"),
            schema_version: 1,
        })
    }

    fn capture_system_store(&self) -> Result<u64, PortError> {
        if let Some(hook) = self.on_capture_system.lock().expect("lock").as_ref() {
            hook(Path::new(""));
        }
        if self.faults.capture_system_error {
            return Err(PortError::SystemStore(std::io::Error::other(
                "system store flush failed",
            )));
        }
        Ok(1)
    }

    fn seal_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        if let Some(hook) = self.on_seal.lock().expect("lock").as_ref() {
            hook(Path::new(""));
        }
        if self.faults.seal_error {
            return Err(PortError::Seal {
                repo: repo.to_string(),
                source: std::io::Error::other("seal failed"),
            });
        }
        Ok(CapturedPoint {
            wal_position: 2,
            store_dir: format!("repos/{repo}"),
            schema_version: 1,
        })
    }

    fn end_seal(&self, _repo: &str) {
        if let Some(hook) = self.on_end_seal.lock().expect("lock").as_ref() {
            hook(Path::new(""));
        }
    }

    fn release_pins(&self) {
        let held = *self.pins_held.lock().expect("lock");
        self.released_from.store(held, Ordering::SeqCst);
        *self.pins_held.lock().expect("lock") = 0;
        self.pins_released.fetch_add(1, Ordering::SeqCst);
    }
}

/// One test fixture: RAII temp dirs for the source and the destination
/// root (dropped at the end of the test, no manual cleanup).
pub(crate) struct Fixture {
    _source: TempDir,
    _to: TempDir,
}

impl Fixture {
    pub(crate) fn new() -> (Self, PathBuf, PathBuf) {
        let source = TempDir::new().expect("source temp dir");
        let to = TempDir::new().expect("destination temp dir");
        InjectPort::source_with_data(source.path());
        let source_path = source.path().to_path_buf();
        let to_path = to.path().to_path_buf();
        let fixture = Self {
            _source: source,
            _to: to,
        };
        (fixture, source_path, to_path)
    }
}

/// Runs the protocol and returns the full report on success (the
/// destination snapshot directory and counters) plus the progress
/// snapshot the status operation would report.
pub(crate) fn run(
    port: &InjectPort,
    source: &Path,
    to: &Path,
    cancel: &CancelFlag,
) -> (Result<BackupReport, OnlineBackupError>, BackupProgress) {
    let mut progress = BackupProgress::new();
    let result = run_online_backup(
        port,
        source,
        to,
        cancel,
        &mut progress,
        &CopyOptions::default(),
        &SealRecopyCap::default(),
    );
    (result, progress)
}

/// The typed failure cause of a failed run, extracted for exact
/// assertion (transparent thiserror wrappers flatten through
/// `source()`, so the chain walk is not typed — match and display the
/// inner failure directly).
fn failure_of(error: &OnlineBackupError) -> &OnlineBackupFailure {
    let OnlineBackupError::Failed { source, .. } = error else {
        panic!("expected Failed, got {error:?}");
    };
    source
}

/// Shared R4 assertions for a failed run: destination root empty, pins
/// back to zero (released), progress carries the exact failed stage and
/// the stage-level error text.
fn assert_cleanup<'a>(
    to: &Path,
    port: &InjectPort,
    progress: &BackupProgress,
    stage: BackupStage,
    error: &'a OnlineBackupError,
    cause_needle: &str,
) -> &'a OnlineBackupFailure {
    assert_eq!(
        fs::read_dir(to).expect("to dir").count(),
        0,
        "R4: no partial output may survive a failed run"
    );
    assert_eq!(
        port.pins_currently_held(),
        0,
        "R4: the truncation pins must be released (counter reset to zero)"
    );
    assert_eq!(
        port.pins_released.load(Ordering::SeqCst),
        1,
        "R4: the pin release must run exactly once"
    );
    assert_eq!(progress.stage, BackupStage::Failed);
    assert_eq!(progress.failed_stage, Some(stage));
    let last_error = progress.last_error.as_deref().expect("last_error");
    assert_eq!(
        last_error,
        error.to_string(),
        "progress must carry the protocol error's top-level display"
    );
    assert_eq!(
        error.stage(),
        stage,
        "the protocol error must carry the actual failed stage"
    );
    let failure = failure_of(error);
    let cause = failure.to_string();
    assert!(
        cause.contains(cause_needle),
        "the failure must carry the actual cause {cause_needle:?}, got: {cause}"
    );
    failure
}

#[test]
fn a_catalogue_error_fails_at_preparing() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let port = InjectPort::single_repo(&source).with_faults(Faults {
        catalogue_error: true,
        ..Faults::default()
    });
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let failure = assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Preparing,
        &error,
        // NOTE: `PortError::SystemStore`'s display is the fixed type text
        // "system store flush failed" — the concrete catalogue cause is
        // only reachable through the nested `io::Error` source, asserted
        // below.
        "system store flush failed",
    );
    assert!(
        matches!(
            failure,
            OnlineBackupFailure::Port(PortError::SystemStore(_))
        ),
        "the cause must be the typed port error, got {failure:?}"
    );
    let OnlineBackupFailure::Port(PortError::SystemStore(io)) = failure else {
        panic!("the cause must be the typed SystemStore port error with a nested io source");
    };
    assert_eq!(
        io.to_string(),
        "catalogue read failed",
        "the nested I/O source must carry the actual catalogue cause"
    );
    assert_source_untouched(&before, &source);
}

/// Pin acquisition fails with a pin PARTIALLY held: two repos, the
/// second acquisition failing AFTER the first pin was acquired, so one
/// pin is genuinely held when the failure propagates. The pre-release
/// count is established through the port's `released_from` record
/// (captured inside `release_pins` before the reset); after the run the
/// held count is zero and the release ran exactly once. REPLACES the
/// previous single-repo no-pin-acquired failure case (the orchestrator
/// decision: the partial model subsumes it; the plain no-pin behaviour
/// remains observable in the fixture, which starts every run at zero).
#[test]
fn a_partial_pin_acquisition_second_of_two_failing_after_first_acquired_releases_all_pins() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let port = InjectPort::two_repos(&source).with_faults(Faults {
        acquire_pins_partial_error: true,
        ..Faults::default()
    });
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let failure = assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Preparing,
        &error,
        "WAL truncation pin acquisition failed",
    );
    assert!(
        matches!(failure, OnlineBackupFailure::Port(PortError::PinAcquire)),
        "the cause must be the typed pin error, got {failure:?}"
    );
    assert_eq!(
        port.released_from.load(Ordering::SeqCst),
        1,
        "the first of two pins was acquired and still held when release ran"
    );
    assert_eq!(
        port.pins_currently_held(),
        0,
        "the partially acquired pin must be released by cleanup"
    );
    assert_source_untouched(&before, &source);
}

#[test]
fn b_capture_flush_error_fails_at_captured_and_leaves_no_output() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let port = InjectPort::single_repo(&source).with_faults(Faults {
        capture_error: true,
        ..Faults::default()
    });
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let failure = assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Captured,
        &error,
        "capture barrier failed for repo alpha",
    );
    assert!(
        matches!(
            failure,
            OnlineBackupFailure::Port(PortError::Capture { repo, .. })
                if repo == "alpha"
        ),
        "the cause must be the typed capture error for alpha, got {failure:?}"
    );
    assert_source_untouched(&before, &source);
}

#[test]
fn b_system_store_flush_error_fails_at_captured() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let port = InjectPort::single_repo(&source).with_faults(Faults {
        capture_system_error: true,
        ..Faults::default()
    });
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let failure = assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Captured,
        &error,
        "system store flush failed",
    );
    assert!(
        matches!(
            failure,
            OnlineBackupFailure::Port(PortError::SystemStore(_))
        ),
        "the cause must be the typed port error, got {failure:?}"
    );
    assert_source_untouched(&before, &source);
}

#[test]
fn d_store_copy_open_failure_at_destination_fails_at_copying_and_cleans_up() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    // Fixture act: during the capture barrier, plant a DIRECTORY at the
    // destination snapshot path of alpha's store file (create_dir_all on
    // the file path itself). The WAL pass (step C) then completes
    // untouched, and the store copy pass (step D) fails when the copy
    // engine's File::create on that exact destination path errors
    // (is-a-directory) before copying any bytes.
    let dest_for_hook = to.clone();
    let dest_path = Arc::new(Mutex::new(None::<PathBuf>));
    let dest_path_for_hook = Arc::clone(&dest_path);
    let port = InjectPort::single_repo(&source).with_capture_hook(move |_| {
        let snapshot = snapshot_dir_of(&dest_for_hook);
        // The destination file the copy pass will open: the same
        // `<snapshot>/repos/alpha/store.bin` path planted below.
        let store_dest = snapshot.join("repos/alpha/store.bin");
        *dest_path_for_hook.lock().expect("dest path lock") = Some(store_dest.clone());
        fs::create_dir_all(store_dest).expect("plant destination blocker directory");
    });
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let OnlineBackupError::Failed {
        source: failure, ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    let OnlineBackupFailure::Copy(CopyError::Dest { path, .. }) = failure else {
        panic!("the failure must be a typed copy-destination error, got {failure:?}");
    };
    let expected_dest = dest_path
        .lock()
        .expect("dest path lock")
        .clone()
        .expect("the capture hook must have recorded the destination path");
    assert_eq!(
        path, &expected_dest,
        "CopyError::Dest must name the destination file whose open failed"
    );
    assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Copying,
        &error,
        "destination write failed for",
    );
    assert_source_untouched(&before, &source);
}

#[test]
fn e_seal_error_fails_at_sealing_with_the_wal_consistent() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let port = InjectPort::single_repo(&source).with_faults(Faults {
        seal_error: true,
        ..Faults::default()
    });
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let failure = assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Sealing,
        &error,
        "seal barrier failed for repo alpha",
    );
    assert!(
        matches!(
            failure,
            OnlineBackupFailure::Port(PortError::Seal { repo, .. }) if repo == "alpha"
        ),
        "the cause must be the typed seal error for alpha, got {failure:?}"
    );
    assert_source_untouched(&before, &source);
}

#[test]
fn f_self_verify_hash_mismatch_removes_output_and_leaves_source_untouched() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    // Fixture act: corrupt an already-final copied DEST file while the
    // second repo's seal barrier runs (alpha's re-copy and end_seal are
    // done, so alpha's destination bytes are final; beta's re-copy does
    // not touch alpha's files). The manifest's alpha hash then no longer
    // matches the snapshot and the step-F self-verification fails.
    let seal_calls = Arc::new(AtomicUsize::new(0));
    let calls_for_hook = seal_calls.clone();
    let dest_for_hook = to.clone();
    let port = InjectPort::two_repos(&source).with_seal_hook(move |_| {
        if calls_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            let snapshot = snapshot_dir_of(&dest_for_hook);
            fs::write(
                snapshot.join("repos/alpha/store.bin"),
                b"corrupted-after-copy",
            )
            .expect("corrupt the already-copied destination file");
        }
    });
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let failure = assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Verifying,
        &error,
        "checksum mismatch",
    );
    assert!(
        matches!(failure, OnlineBackupFailure::Verify(_)),
        "the cause must be the typed self-verification error, got {failure:?}"
    );
    assert_source_untouched(&before, &source);
}

#[test]
fn f_manifest_write_failure_fails_at_verifying_with_no_success_report() {
    let (_fixture, source, to) = Fixture::new();
    // Fixture act: a top-level source directory named `manifest.json.tmp`
    // holding a blocker file. The copy passes materialise it as a
    // DIRECTORY at the destination, and `write_manifest` writes its tmp
    // file at exactly that name before renaming to `manifest.json`, so
    // the create of the tmp file itself fails (is-a-directory) — no
    // manifest, and no success report, can ever be written. The failure
    // is a DESTINATION failure (`OnlineBackupFailure::Dest`), never a
    // generic io error: the published manifest is the protocol's own
    // output, not anything read from the source.
    write_file(&source.join("manifest.json.tmp/blocker"), b"blocker");
    let before = snapshot_dir(&source);
    let port = InjectPort::two_repos(&source);
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    let failure = assert_cleanup(
        &to,
        &port,
        &progress,
        BackupStage::Verifying,
        &error,
        "destination publication failed for",
    );
    let OnlineBackupFailure::Publication { source: io, .. } = failure else {
        panic!("the manifest failure must surface as a publication failure, got {failure:?}");
    };
    assert!(
        !io.to_string().is_empty(),
        "the destination failure must carry the io cause"
    );
    let rendered = error.stage_and_reason();
    assert!(
        rendered.starts_with("verifying: destination-io-"),
        "the scheduled reason must name the destination, got: {rendered}"
    );
    assert!(
        !rendered.contains('/') && !rendered.contains('\\'),
        "the rendering must stay path-free: {rendered}"
    );
    assert_source_untouched(&before, &source);
}

#[test]
fn g_cancel_at_the_captured_boundary_cleans_up_and_releases_pins() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let port = InjectPort::single_repo(&source).with_pins_hook(move |_| {
        // Cancel exactly at the boundary between step A (pins held) and
        // the step-B entry check: the next boundary check observes it.
        cancel_for_hook.cancel();
    });
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    assert!(
        matches!(
            error,
            OnlineBackupError::Cancelled {
                stage: BackupStage::Captured,
                ..
            }
        ),
        "expected Cancelled(Captured), got {error:?}"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    // record_failure tags a cancellation too: the failed stage IS the
    // cancelled stage.
    assert_eq!(
        progress.failed_stage,
        Some(BackupStage::Captured),
        "the cancelled stage must be reported as failed_stage",
    );
    assert_eq!(progress.stage, BackupStage::Failed);
    assert_eq!(
        progress.last_error.as_deref(),
        Some("backup cancelled at stage Captured"),
        "progress must carry the cancellation"
    );
    assert_source_untouched(&before, &source);
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "shamir_online_failure_{name}_{}_{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

struct CancelDuringValidation<'a> {
    inner: &'a TestPort,
    cancel: &'a CancelFlag,
}

impl OnlineBackupPort for CancelDuringValidation<'_> {
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError> {
        self.inner.catalogue()
    }
    fn acquire_pins(&self) -> Result<(), PortError> {
        self.inner.acquire_pins()
    }
    fn capture_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        self.inner.capture_point(repo)
    }
    fn capture_system_store(&self) -> Result<u64, PortError> {
        self.inner.capture_system_store()
    }
    fn seal_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        self.inner.seal_point(repo)
    }
    fn end_seal(&self, repo: &str) {
        self.inner.end_seal(repo)
    }
    fn release_pins(&self) {
        self.inner.release_pins()
    }
    fn validate_copied_data_dir(&self, _copied_data_dir: &Path) -> Result<(), PortError> {
        self.cancel.cancel();
        Ok(())
    }
}

#[test]
fn run_publishes_no_manifest_when_cancel_fires_during_validation() {
    let source = temp_dir("cancel_validation_source");
    let to = temp_dir("cancel_validation_to");
    TestPort::source_with_data(&source);
    let inner = TestPort::single_repo(&source);
    let cancel = CancelFlag::default();
    let wrapper = CancelDuringValidation {
        inner: &inner,
        cancel: &cancel,
    };
    let mut progress = BackupProgress::new();
    let result = run_online_backup(
        &wrapper,
        &source,
        &to,
        &cancel,
        &mut progress,
        &CopyOptions::default(),
        &SealRecopyCap::default(),
    );
    assert!(matches!(
        result,
        Err(OnlineBackupError::Cancelled {
            stage: BackupStage::Verifying,
            ..
        })
    ));
    assert_eq!(fs::read_dir(&to).expect("destination").count(), 0);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn g_cancel_inside_end_seal_fails_at_sealing_cleans_up_and_releases_pins() {
    // A cancel set inside the last `end_seal` is observed during the
    // seal re-copy itself: `copy_engine` checks the cancel flag BEFORE
    // per-file exclusion, so with alpha still present the residual pass
    // trips in Sealing — there is NO F (Verifying) boundary reachable
    // without a seam. The engine drops the partial destination; cleanup
    // then runs and the cancelled stage is reported as failed_stage.
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let port = InjectPort::single_repo(&source).with_end_seal_hook(move |_| {
        cancel_for_hook.cancel();
    });
    let (result, progress) = run(&port, &source, &to, &cancel);
    let error = result.expect_err("must fail");
    assert!(
        matches!(
            error,
            OnlineBackupError::Cancelled {
                stage: BackupStage::Sealing,
                ..
            }
        ),
        "expected Cancelled(Sealing), got {error:?}"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    // record_failure tags a cancellation too: the failed stage IS the
    // cancelled stage.
    assert_eq!(
        progress.failed_stage,
        Some(BackupStage::Sealing),
        "the cancelled stage must be reported as failed_stage",
    );
    assert_eq!(progress.stage, BackupStage::Failed);
    assert_eq!(
        progress.last_error.as_deref(),
        Some("backup cancelled at stage Sealing"),
        "progress must carry the cancellation"
    );
    assert_source_untouched(&before, &source);
}
