//! Protocol state machine tests over a plain-directory port (CI only).

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::sync::durability::backup::online::copy_engine::{
    test_seams::{self, SeamGuard},
    CopyOptions,
};
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupPermit, BackupReport, OnlineBackupError, OnlineBackupFailure,
    SealRecopyCap,
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

/// Recorded seal/end_seal ordering over the run (design 1d (b)).
type Events = Mutex<Vec<String>>;

/// Port over plain directories; capture/seal hooks inject mid-protocol
/// mutations and cancellations. Pin acquire/release are counted; seal and
/// end_seal calls are recorded in order.
struct TestPort {
    repos: Vec<RepoSource>,
    pins_held: AtomicUsize,
    pins_released: AtomicUsize,
    on_capture: Mutex<Option<Hook>>,
    on_seal: Mutex<Option<Hook>>,
    on_end_seal: Mutex<Option<Hook>>,
    events: Events,
}

impl TestPort {
    fn single_repo(source: &Path) -> Self {
        Self::with_repos(source, &["alpha"])
    }

    fn two_repos(source: &Path) -> Self {
        Self::with_repos(source, &["alpha", "beta"])
    }

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
            pins_held: AtomicUsize::new(0),
            pins_released: AtomicUsize::new(0),
            on_capture: Mutex::new(None),
            on_seal: Mutex::new(None),
            on_end_seal: Mutex::new(None),
            events: Mutex::new(Vec::new()),
        }
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

    fn with_capture_hook(self, hook: impl Fn() + Send + 'static) -> Self {
        *self.on_capture.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    fn with_seal_hook(self, hook: impl Fn() + Send + 'static) -> Self {
        *self.on_seal.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    fn with_end_seal_hook(self, hook: impl Fn() + Send + 'static) -> Self {
        *self.on_end_seal.lock().expect("lock") = Some(Box::new(hook));
        self
    }

    fn source_with_data(source: &Path) {
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

impl OnlineBackupPort for TestPort {
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError> {
        Ok(self.repos.clone())
    }

    fn acquire_pins(&self) -> Result<(), PortError> {
        self.pins_held.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn capture_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        if let Some(hook) = self.on_capture.lock().expect("lock").as_ref() {
            hook();
        }
        Ok(CapturedPoint {
            wal_position: 1,
            store_dir: format!("repos/{repo}"),
            schema_version: 1,
        })
    }

    fn capture_system_store(&self) -> Result<u64, PortError> {
        Ok(1)
    }

    fn seal_point(&self, repo: &str) -> Result<CapturedPoint, PortError> {
        self.events
            .lock()
            .expect("lock")
            .push(format!("seal:{repo}"));
        if let Some(hook) = self.on_seal.lock().expect("lock").as_ref() {
            hook();
        }
        Ok(CapturedPoint {
            wal_position: 2,
            store_dir: format!("repos/{repo}"),
            schema_version: 1,
        })
    }

    fn end_seal(&self, repo: &str) {
        self.events
            .lock()
            .expect("lock")
            .push(format!("end_seal:{repo}"));
        if let Some(hook) = self.on_end_seal.lock().expect("lock").as_ref() {
            hook();
        }
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
    run_with_cap(port, source, to, cancel, &SealRecopyCap::default())
}

fn run_with_cap(
    port: &TestPort,
    source: &Path,
    to: &Path,
    cancel: &CancelFlag,
    cap: &SealRecopyCap,
) -> Result<BackupReport, OnlineBackupError> {
    let mut progress = BackupProgress::new();
    run_online_backup(
        port,
        source,
        to,
        cancel,
        &mut progress,
        &CopyOptions::default(),
        cap,
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
    verify_manifest(&report.dest_dir).expect("snapshot must self-verify");
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
    verify_manifest(&report.dest_dir).expect("snapshot must verify");
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
    // beta is a non-catalogue directory, copied without barriers — only
    // alpha's deleted file must be absent.
    assert!(
        !manifest.contains("repos/alpha/store.bin"),
        "deleted file must not be in the manifest"
    );
    verify_manifest(&report.dest_dir).expect("snapshot must verify");
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

    // Hook cancels during the seal barrier; the seal re-copy observes it.
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
            stage: BackupStage::Sealing
        }
    ));
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(
        port.seal_events(),
        vec!["seal:alpha".to_string(), "end_seal:alpha".to_string()],
        "cancellation mid-seal must end the open hold"
    );
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

#[test]
fn seal_hold_lives_exactly_through_each_repo_recopy() {
    let source = temp_dir("order_source");
    let to = temp_dir("order_to");
    TestPort::source_with_data(&source);
    let port = TestPort::two_repos(&source);
    let cancel = CancelFlag::default();
    run(&port, &source, &to, &cancel).expect("run");
    assert_eq!(
        port.seal_events(),
        vec![
            "seal:alpha".to_string(),
            "end_seal:alpha".to_string(),
            "seal:beta".to_string(),
            "end_seal:beta".to_string(),
        ],
        "each repo's hold must end after its re-copy, before the next seal"
    );
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn byte_cap_exceeded_fails_closed_and_cleans_up() {
    let source = temp_dir("cap_source");
    let to = temp_dir("cap_to");
    TestPort::source_with_data(&source);
    // Non-vacuous oracle: alpha's store CHANGES at the seal barrier, so
    // the seal re-copy must actually write (> 1 byte) and the cap of
    // one byte must trip. With an unchanged source the re-copy pass
    // would skip every file and the cap could never fire.
    let store = source.join("repos/alpha/store.bin");
    let port = TestPort::two_repos(&source).with_seal_hook(move || {
        fs::write(&store, b"mutated-at-seal").expect("mutate store at seal");
    });
    let cancel = CancelFlag::default();
    let cap = SealRecopyCap {
        max_bytes: Some(1),
        max_seconds: None,
    };
    let error = run_with_cap(&port, &source, &to, &cancel, &cap).expect_err("must fail");
    let OnlineBackupError::Failed {
        stage,
        source: failure,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert!(
        matches!(failure, OnlineBackupFailure::RecopyCap(_)),
        "cap failure must be the source error"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    let events = port.seal_events();
    assert_eq!(
        events.last(),
        Some(&"end_seal:alpha".to_string()),
        "the open hold of the failing repo must be ended"
    );
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn unchanged_sources_complete_even_under_a_one_byte_cap() {
    // The cap fires only on re-copy WRITES: with no seal-barrier change
    // the re-copy pass skips every file and the run completes even at
    // one byte — the two-sided baseline for the cap-trip test above.
    let source = temp_dir("cap_untouched_source");
    let to = temp_dir("cap_untouched_to");
    TestPort::source_with_data(&source);
    let scope = SealScope::new();
    let port = TestPort::two_repos(&source)
        .with_seal_hook(scope.set_active())
        .with_end_seal_hook(scope.clear_active());
    let _seam = observe_writes(&scope);
    let cancel = CancelFlag::default();
    let cap = SealRecopyCap {
        max_bytes: Some(1),
        max_seconds: None,
    };
    let report = run_with_cap(&port, &source, &to, &cancel, &cap).expect("run must succeed");
    verify_manifest(&report.dest_dir).expect("snapshot must verify");
    assert!(
        scope.log().iter().all(|entry| !entry.starts_with("seal:")),
        "an unchanged source must trigger no seal re-copy writes: {:?}",
        scope.log()
    );
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn clock_cap_exceeded_fails_closed() {
    let source = temp_dir("clock_cap_source");
    let to = temp_dir("clock_cap_to");
    TestPort::source_with_data(&source);
    let port = TestPort::single_repo(&source);
    let cancel = CancelFlag::default();
    let cap = SealRecopyCap {
        max_bytes: None,
        max_seconds: Some(0),
    };
    let error = run_with_cap(&port, &source, &to, &cancel, &cap).expect_err("must fail");
    assert!(matches!(
        error,
        OnlineBackupError::Failed {
            stage: BackupStage::Sealing,
            ..
        }
    ));
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

// --- Corrective-delta (seal re-copy) regression tests -----------------
//
// These use only the existing seams: the `TestPort` capture/seal/end_seal
// hooks and `copy_engine::test_seams` (`after_first_write` fires per
// completed chunk write, including the gated seal re-copy). A test-local
// `SealScope.active` flag scopes observation to the seal re-copy pass:
// set by the seal hook, cleared by the end_seal hook.

/// Copy-engine chunk size: every `after_first_write` fire is one chunk.
const CHUNK: u64 = 1024 * 1024;

/// Deterministic content strictly larger than two chunks: the byte cap
/// below admits exactly the FIRST chunk and must trip DURING the second.
fn big_store_bytes() -> Vec<u8> {
    let capacity = 2 * CHUNK as usize + 4096;
    let mut bytes = Vec::with_capacity(capacity);
    for index in 0..capacity {
        bytes.push((index % 251) as u8);
    }
    bytes
}

/// Shared observation state of the seal re-copy: the active flag (set at
/// the seal barrier, cleared at end_seal) and the per-chunk write log.
struct SealScope {
    active: Arc<AtomicBool>,
    writes: Arc<Mutex<Vec<String>>>,
}

impl SealScope {
    fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            writes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn set_active(&self) -> impl Fn() + Send + 'static {
        let active = Arc::clone(&self.active);
        move || active.store(true, Ordering::SeqCst)
    }

    fn clear_active(&self) -> impl Fn() + Send + 'static {
        let active = Arc::clone(&self.active);
        move || active.store(false, Ordering::SeqCst)
    }

    fn log(&self) -> Vec<String> {
        self.writes.lock().expect("lock").clone()
    }
}

/// Registers the write-observation seam scoped to `scope`'s seal phase.
fn observe_writes(scope: &SealScope) -> SeamGuard {
    let active = Arc::clone(&scope.active);
    let writes = Arc::clone(&scope.writes);
    test_seams::set_after_first_write(Box::new(move |source: &Path| {
        let phase = if active.load(Ordering::SeqCst) {
            "seal"
        } else {
            "first"
        };
        writes.lock().expect("lock").push(format!(
            "{phase}:{}",
            // Normalized separators: assertions match `repos/alpha/...`
            // on every platform.
            source.to_string_lossy().replace('\\', "/")
        ));
        Ok(())
    }))
}

fn count_containing(log: &[String], phase: &str, needle: &str) -> usize {
    log.iter()
        .filter(|entry| entry.starts_with(&format!("{phase}:")) && entry.contains(needle))
        .count()
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in Sha256::digest(bytes) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[test]
fn seal_recopy_of_a_new_big_file_is_capped_during_the_second_chunk() {
    let source = temp_dir("cap_mid_source");
    let to = temp_dir("cap_mid_to");
    TestPort::source_with_data(&source);
    let big = big_store_bytes();
    let store = source.join("repos/alpha/store.bin");
    let scope = SealScope::new();
    // First-pass unchanged assertion + corrective delta, both through
    // the per-repo seal hook: at the seal barrier the live source STILL
    // holds the pre-point bytes the first passes copied, then the hook
    // replaces the same-path store file with the >2-chunk content.
    let store_for_hook = store.clone();
    let big_for_hook = big.clone();
    let first_pass_store_unchanged = Arc::new(AtomicBool::new(false));
    let unchanged_flag = Arc::clone(&first_pass_store_unchanged);
    let active_for_hook = Arc::clone(&scope.active);
    let port = TestPort::single_repo(&source)
        .with_seal_hook(move || {
            let current = fs::read(&store_for_hook).expect("read live store");
            unchanged_flag.store(current == b"store-bytes", Ordering::SeqCst);
            fs::write(&store_for_hook, &big_for_hook).expect("replace store at seal");
            active_for_hook.store(true, Ordering::SeqCst);
        })
        .with_end_seal_hook(scope.clear_active());
    let _seam = observe_writes(&scope);
    let cancel = CancelFlag::default();
    // Cap admits the ~11 bytes of the WAL re-pass plus exactly ONE chunk.
    let cap = SealRecopyCap {
        max_bytes: Some(CHUNK + 512),
        max_seconds: None,
    };
    let error = run_with_cap(&port, &source, &to, &cancel, &cap).expect_err("must fail");
    let OnlineBackupError::Failed {
        stage,
        source: failure,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert!(
        matches!(failure, OnlineBackupFailure::RecopyCap(_)),
        "cap failure must be the source error, got {failure:?}"
    );

    // First pass copied the unchanged WAL and store exactly once each.
    assert!(
        first_pass_store_unchanged.load(Ordering::SeqCst),
        "seal hook must observe the first-pass source unchanged"
    );
    let log = scope.log();
    assert_eq!(
        count_containing(&log, "first", "repos/alpha"),
        3,
        "WAL meta + WAL segment + store, copied unchanged once each: {log:?}"
    );
    assert_eq!(
        count_containing(&log, "first", "repos/alpha/store.bin"),
        1,
        "first pass must copy the store once"
    );
    // The corrective delta was capped DURING the second chunk: exactly
    // the first chunk of the big file reached the destination.
    assert_eq!(
        count_containing(&log, "seal", "repos/alpha/store.bin"),
        1,
        "exactly the first chunk may be copied before the cap trips, log {log:?}"
    );

    // Fail-closed cleanup: no output, pins released, hold ended, and
    // the live source keeps the corrective delta untouched.
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(
        port.seal_events().last(),
        Some(&"end_seal:alpha".to_string()),
        "the open hold of the failing repo must be ended"
    );
    assert_eq!(fs::read(&store).expect("live store"), big);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn wal_cut_staged_snapshot_excludes_frames_appended_after_end_seal() {
    let source = temp_dir("wal_cut_source");
    let to = temp_dir("wal_cut_to");
    let first_pass = b"F0;";
    let at_seal = b"F0;F1;";
    let post_point = b"F2;post-point;";
    write_file(
        &source.join("repos/alpha.shamirwal/00000000.wal"),
        first_pass,
    );
    write_file(
        &source.join("repos/alpha.shamirwal/00000000.meta"),
        b"meta-0",
    );
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    let to_for_hook = to.clone();
    let staged: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let staged_for_hook = Arc::clone(&staged);
    let wal_for_hook = source.join("repos/alpha.shamirwal/00000000.wal");
    let seal_for_hook = source.join("repos/alpha.shamirwal/00000000.wal");
    let post_for_hook = post_point.to_vec();
    // The writer appends AT the seal barrier (F1) and again ONLY from
    // end_seal (F2): while the hold is live, nothing can append; the
    // moment end_seal drops the hold, the writer lands its frames on
    // the LIVE wal. Because the wal CHANGES at the seal barrier, the
    // staged snapshot can hold F1 only if the seal re-copy re-reads it.
    let port = TestPort::single_repo(&source)
        .with_seal_hook(move || {
            let mut live = fs::OpenOptions::new()
                .append(true)
                .open(&seal_for_hook)
                .expect("open live wal");
            live.write_all(b"F1;").expect("append seal frame");
        })
        .with_end_seal_hook(move || {
            let mut live = fs::OpenOptions::new()
                .append(true)
                .open(&wal_for_hook)
                .expect("open live wal");
            live.write_all(&post_for_hook).expect("append post-point");
            // Staged snapshot: the sole timestamp dir under `to`, already
            // final for this repo (the re-copy completed before end_seal).
            let snapshot = fs::read_dir(&to_for_hook)
                .expect("to dir")
                .filter_map(|entry| entry.ok())
                .next()
                .expect("snapshot dir")
                .path();
            *staged_for_hook.lock().expect("lock") = Some(
                fs::read(snapshot.join("repos/alpha.shamirwal/00000000.wal")).expect("staged wal"),
            );
        });
    let cancel = CancelFlag::default();
    let report = run(&port, &source, &to, &cancel).expect("run must succeed");

    // The hold WAS live through the WAL re-copy pass: the staged
    // snapshot holds exactly the seal-barrier frames. If the re-copy
    // were skipped, the staged bytes would still be the first-pass F0.
    assert_eq!(
        staged.lock().expect("lock").as_deref(),
        Some(&at_seal[..]),
        "staged bytes must be the seal-barrier content, not pre-seal F0"
    );
    // ...while the live source contains them.
    let live = fs::read(source.join("repos/alpha.shamirwal/00000000.wal")).expect("live wal");
    assert!(
        live.ends_with(post_point),
        "live source must contain the post-point frames"
    );
    // The manifest defines the snapshot from the STAGED bytes, never
    // from the live file: the recorded segment hash is the seal-time
    // digest (real content hashes — no fake version-field check).
    let expected_hash = hex_digest(at_seal);
    let manifest = fs::read_to_string(report.dest_dir.join("manifest.json")).expect("manifest");
    assert!(
        manifest.contains(&expected_hash),
        "manifest must hash the staged seal-time content ({expected_hash})"
    );
    assert!(
        !manifest.contains(&hex_digest(&live)),
        "manifest must not hash the live post-point content"
    );
    verify_manifest(&report.dest_dir).expect("snapshot must self-verify");
    assert_eq!(
        port.seal_events(),
        vec!["seal:alpha".to_string(), "end_seal:alpha".to_string()]
    );
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn clock_cap_trips_mid_copy_through_the_monotonic_seam() {
    let source = temp_dir("clock_mid_source");
    let to = temp_dir("clock_mid_to");
    TestPort::source_with_data(&source);
    let big = big_store_bytes();
    let store = source.join("repos/alpha/store.bin");
    let scope = SealScope::new();
    let store_for_hook = store.clone();
    let big_for_hook = big.clone();
    let active_for_hook = Arc::clone(&scope.active);
    let port = TestPort::single_repo(&source)
        .with_seal_hook(move || {
            fs::write(&store_for_hook, &big_for_hook).expect("replace store at seal");
            active_for_hook.store(true, Ordering::SeqCst);
        })
        .with_end_seal_hook(scope.clear_active());
    // Deterministic in-flight clock: once the FIRST chunk of the
    // corrective delta has landed, the gate's monotonic reads report an
    // instant two hours past the real clock — far beyond the 1-hour
    // deadline taken before the seal barrier. No sleeps. One combined
    // hook records the seal-phase chunk writes AND shifts the clock.
    let shifted = Arc::new(AtomicBool::new(false));
    let _seam = {
        let active = Arc::clone(&scope.active);
        let writes = Arc::clone(&scope.writes);
        let shifted = Arc::clone(&shifted);
        test_seams::set_after_first_write(Box::new(move |source: &Path| {
            if active.load(Ordering::SeqCst) && source.ends_with("store.bin") {
                writes.lock().expect("lock").push(format!(
                    "seal:{}",
                    source.to_string_lossy().replace('\\', "/")
                ));
                shifted.store(true, Ordering::SeqCst);
            }
            Ok(())
        }))
    };
    let _clock = {
        let shifted = Arc::clone(&shifted);
        test_seams::set_monotonic_now(Box::new(move |real| {
            if shifted.load(Ordering::SeqCst) {
                real + Duration::from_secs(2 * 3600)
            } else {
                real
            }
        }))
    };
    let cancel = CancelFlag::default();
    let cap = SealRecopyCap {
        max_bytes: None,
        max_seconds: Some(3600),
    };
    let error = run_with_cap(&port, &source, &to, &cancel, &cap).expect_err("must fail");
    let OnlineBackupError::Failed {
        stage,
        source: failure,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert!(
        matches!(failure, OnlineBackupFailure::RecopyCap(_)),
        "the deadline must trip as RecopyCap mid-copy, got {failure:?}"
    );
    assert!(
        shifted.load(Ordering::SeqCst),
        "the trip must have happened in-flight, after the first chunk"
    );
    assert_eq!(
        count_containing(&scope.log(), "seal", "repos/alpha/store.bin"),
        1,
        "exactly the first chunk may land before the clock trips, log {:?}",
        scope.log()
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(
        port.seal_events().last(),
        Some(&"end_seal:alpha".to_string())
    );
    assert_eq!(fs::read(&store).expect("live store"), big);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}

#[test]
fn panic_mid_seal_recopy_ends_the_open_seal_hold_on_unwind() {
    let source = temp_dir("panic_hold_source");
    let to = temp_dir("panic_hold_to");
    TestPort::source_with_data(&source);
    let store = source.join("repos/alpha/store.bin");
    let big = big_store_bytes();
    let scope = SealScope::new();
    let store_for_hook = store.clone();
    let big_for_hook = big.clone();
    let active_for_hook = Arc::clone(&scope.active);
    let port = TestPort::single_repo(&source)
        .with_seal_hook(move || {
            fs::write(&store_for_hook, &big_for_hook).expect("replace store at seal");
            active_for_hook.store(true, Ordering::SeqCst);
        })
        .with_end_seal_hook(scope.clear_active());
    let active = Arc::clone(&scope.active);
    let _seam = test_seams::set_after_first_write(Box::new(move |source: &Path| {
        if active.load(Ordering::SeqCst) && source.ends_with("store.bin") {
            panic!("injected unwind mid-chunk");
        }
        Ok(())
    }));
    let cancel = CancelFlag::default();
    let error = catch_unwind(AssertUnwindSafe(|| run(&port, &source, &to, &cancel)))
        .expect_err("the injected panic must propagate");
    // No panic-hook swapping (it races with parallel tests and swallows
    // other failures' diagnostics): the payload itself must be the
    // injected sentinel.
    let message = error
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| error.downcast_ref::<String>().map(String::as_str))
        .expect("panic payload must be a string");
    assert_eq!(message, "injected unwind mid-chunk");
    // The RAII seal hold must have ended on the unwind path — the
    // per-repo guards of `run_steps` drop before the run cleanup.
    assert_eq!(
        port.seal_events(),
        vec!["seal:alpha".to_string(), "end_seal:alpha".to_string()],
        "the open hold must be ended by the guard's Drop during unwind"
    );
    // The RAII run cleanup must ALSO have run on the unwind path: the
    // owned partial output directory is removed and the pins released —
    // exactly once, no double release, no double panic.
    assert_eq!(port.pins_held.load(Ordering::SeqCst), 1);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(
        fs::read_dir(&to).expect("to dir").count(),
        0,
        "the owned partial destination directory must be removed on unwind"
    );
    assert_eq!(fs::read(&store).expect("live store"), big);
    let _ = fs::remove_dir_all(&source);
    let _ = fs::remove_dir_all(&to);
}
