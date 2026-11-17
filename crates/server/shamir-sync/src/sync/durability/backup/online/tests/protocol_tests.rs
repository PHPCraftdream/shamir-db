//! Protocol state machine tests over a plain-directory port (CI only).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sync::durability::backup::online::copy_engine::CopyOptions;
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupPermit, BackupReport, OnlineBackupError,
};
use crate::sync::durability::backup::online::wal_capture::{
    CapturedPoint, OnlineBackupPort, PortError, RepoSource,
};
use crate::sync::durability::backup::verify_manifest;

/// Unique temp dir per test; cleaned up best-effort.
fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "shamir_online_protocol_{}_{name}_{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    fs::write(path, bytes).expect("write");
}

type Hook = Box<dyn Fn() + Send>;

/// Port over plain directories; capture/seal hooks inject mid-protocol
/// mutations and cancellations. Pin acquire/release are counted.
struct TestPort {
    repos: Vec<RepoSource>,
    pins_held: AtomicUsize,
    pins_released: AtomicUsize,
    on_capture: Mutex<Option<Hook>>,
    on_seal: Mutex<Option<Hook>>,
}

impl TestPort {
    fn single_repo(source: &Path) -> Self {
        Self {
            repos: vec![RepoSource {
                name: "alpha".to_string(),
                store_dir: source.join("repos/alpha"),
                wal_dir: Some(source.join("repos/alpha.shamirwal")),
            }],
            pins_held: AtomicUsize::new(0),
            pins_released: AtomicUsize::new(0),
            on_capture: Mutex::new(None),
            on_seal: Mutex::new(None),
        }
    }

    fn with_capture_hook(self, hook: impl Fn() + Send + 'static) -> Self {
        *self.on_capture.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    fn with_seal_hook(self, hook: impl Fn() + Send + 'static) -> Self {
        *self.on_seal.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    fn source_with_data(source: &Path) {
        write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
        write_file(&source.join("repos/alpha.shamirwal/00000000.wal"), b"wal-0");
        write_file(
            &source.join("repos/alpha.shamirwal/00000000.meta"),
            b"meta-0",
        );
    }
}

impl OnlineBackupPort for TestPort {
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError> {
        Ok(self.repos.clone())
    }

    fn acquire_pins(&self) -> Result<(), PortError> {
        self.pins_held.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn capture_point(&self, _repo: &str) -> Result<CapturedPoint, PortError> {
        if let Some(hook) = self.on_capture.lock().expect("lock").as_ref() {
            hook();
        }
        Ok(CapturedPoint {
            wal_position: 1,
            store_dir: "repos/alpha".to_string(),
            schema_version: 1,
        })
    }

    fn capture_system_store(&self) -> Result<u64, PortError> {
        Ok(1)
    }

    fn seal_point(&self, _repo: &str) -> Result<CapturedPoint, PortError> {
        if let Some(hook) = self.on_seal.lock().expect("lock").as_ref() {
            hook();
        }
        Ok(CapturedPoint {
            wal_position: 2,
            store_dir: "repos/alpha".to_string(),
            schema_version: 1,
        })
    }

    fn release_pins(&self) {
        self.pins_released.fetch_add(1, Ordering::SeqCst);
    }
}

fn run(
    port: &TestPort,
    source: &Path,
    to: &Path,
    cancel: &CancelFlag,
) -> Result<BackupReport, OnlineBackupError> {
    let mut progress = BackupProgress::new();
    run_online_backup(
        port,
        source,
        to,
        cancel,
        &mut progress,
        &CopyOptions::default(),
    )
}

#[test]
fn full_run_produces_a_verifiable_snapshot() {
    let source = temp_dir("full_source");
    let to = temp_dir("full_to");
    TestPort::source_with_data(&source);
    let port = TestPort::single_repo(&source);
    let cancel = CancelFlag::default();
    let report = run(&port, &source, &to, &cancel).expect("run");
    assert!(report.dest_dir.join("manifest.json").is_file());
    assert!(report.dest_dir.join("backup.meta.json").is_file());
    let meta = fs::read_to_string(report.dest_dir.join("backup.meta.json")).expect("meta");
    assert!(meta.contains("\"encryption\": \"none\""));
    assert!(
        verify_manifest(&report.dest_dir).is_ok(),
        "snapshot must self-verify"
    );
    assert_eq!(port.pins_held.load(Ordering::SeqCst), 1);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn file_mutated_at_seal_is_re_copied_into_the_manifest() {
    let source = temp_dir("seal_mut_source");
    let to = temp_dir("seal_mut_to");
    TestPort::source_with_data(&source);
    let store = source.join("repos/alpha/store.bin");
    let port = TestPort::single_repo(&source).with_seal_hook(move || {
        fs::write(&store, b"mutated-at-seal").expect("mutate");
    });
    let cancel = CancelFlag::default();
    let report = run(&port, &source, &to, &cancel).expect("run");
    let copied = fs::read(report.dest_dir.join("repos/alpha/store.bin")).expect("copied file");
    assert_eq!(copied, b"mutated-at-seal", "seal re-copy must win");
    assert!(verify_manifest(&report.dest_dir).is_ok());
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn file_deleted_mid_copy_is_absent_from_the_manifest() {
    let source = temp_dir("deleted_source");
    let to = temp_dir("deleted_to");
    TestPort::source_with_data(&source);
    let store = source.join("repos/alpha/store.bin");
    let port = TestPort::single_repo(&source).with_capture_hook(move || {
        let _ = fs::remove_file(&store);
    });
    let cancel = CancelFlag::default();
    let report = run(&port, &source, &to, &cancel).expect("run");
    let manifest = fs::read_to_string(report.dest_dir.join("manifest.json")).expect("manifest");
    assert!(
        !manifest.contains("store.bin"),
        "deleted file must not be in the manifest"
    );
    assert!(verify_manifest(&report.dest_dir).is_ok());
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn cancellation_at_stage_boundaries_cleans_up_and_releases_pins() {
    // Pre-cancelled: fails in Preparing, leaves no destination.
    let source = temp_dir("cancel_prep_source");
    let to = temp_dir("cancel_prep_to");
    TestPort::source_with_data(&source);
    let port = TestPort::single_repo(&source);
    let cancel = CancelFlag::default();
    cancel.cancel();
    let error = run(&port, &source, &to, &cancel).expect_err("must fail");
    assert!(matches!(
        error,
        OnlineBackupError::Cancelled {
            stage: BackupStage::Preparing
        }
    ));
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);

    // Hook cancels during the capture barrier.
    let source = temp_dir("cancel_cap_source");
    let to = temp_dir("cancel_cap_to");
    TestPort::source_with_data(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let port = TestPort::single_repo(&source).with_capture_hook(move || {
        cancel_for_hook.cancel();
    });
    let error = run(&port, &source, &to, &cancel).expect_err("must fail");
    assert!(matches!(
        error,
        OnlineBackupError::Cancelled {
            stage: BackupStage::Copying
        }
    ));
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);

    // Hook cancels during the seal barrier.
    let source = temp_dir("cancel_seal_source");
    let to = temp_dir("cancel_seal_to");
    TestPort::source_with_data(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let port = TestPort::single_repo(&source).with_seal_hook(move || {
        cancel_for_hook.cancel();
    });
    let error = run(&port, &source, &to, &cancel).expect_err("must fail");
    assert!(matches!(
        error,
        OnlineBackupError::Cancelled {
            stage: BackupStage::Verifying
        }
    ));
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn a_second_concurrent_run_is_refused() {
    let permit = BackupPermit::new();
    let first = permit.try_acquire();
    assert!(first.is_some(), "first acquire must succeed");
    assert!(permit.try_acquire().is_none(), "second must be refused");
    drop(first);
    assert!(
        permit.try_acquire().is_some(),
        "release must free the permit"
    );
}
