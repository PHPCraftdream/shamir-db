use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_private_dir::create_private_dir;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};
use tokio::sync::oneshot;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::{
    self, ConsistencySection, RepoConsistency, WalDirEntry, WalSection, WalSegmentEntry,
    BACKUP_META_FILE_NAME,
};
use crate::sync::durability::backup::online::progress::BackupStage;
use crate::sync::durability::backup::{
    verify_manifest, ManifestFileEntry, MANIFEST_FILE_NAME, SNAPSHOT_LEASE_FILE_NAME,
};

use super::super::super::metrics::backup_metrics_snapshot;
use super::super::super::registry::arm_scheduled_retention_pause;
use super::super::super::{OnlineBackupRegistry, StartBackupError};

const OWNER: &str = "registry-test-owner";
const WAIT: Duration = Duration::from_secs(120);
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Fixed names/creation times the test-only post-run callbacks pin each
/// run's output to: both are deterministically OLDER than any real run
/// (real nanosecond timestamps are ~1e18) and ordered 1 < 2, so run 2's
/// prune deletes exactly run 1's copy beyond `keep_last = 1` regardless of
/// wall clock.
const RUN1_NAME: &str = "20200101_000003";
const RUN1_CREATED_AT: u128 = 1;
const RUN2_NAME: &str = "20200101_000004";
const RUN2_CREATED_AT: u128 = 2;

/// Await a scheduled run's LATE signal: `finished` resolves only after the
/// retention pass and the final record publication (`running = false`), so
/// every later assertion observes the run's end. Bounded; no polling and no
/// sleep.
async fn await_finished(finished: oneshot::Receiver<()>) {
    tokio::time::timeout(WAIT, finished)
        .await
        .expect("the run must finish (retention + final publication) within the bound")
        .expect("the runner must send its late FINISHED signal rather than dropping it");
}

/// Rename the just-published snapshot to `fixed_name` and rewrite its
/// manifest creation time to `created_at` (`manifest::decode`/`encode`, so
/// flattened unknown fields survive; the file already exists, so a direct
/// `fs::write` replaces it — no temp/renamed replacement path). The
/// registry captures the adoption digest AFTER this runs, so the recorded
/// provenance binds to the final bytes at the final name.
fn pin_fixture(published: &Path, fixed_name: &str, created_at: u128) -> Option<PathBuf> {
    let fixed = published.parent()?.join(fixed_name);
    fs::rename(published, &fixed)
        .unwrap_or_else(|error| panic!("pin {published:?} to {fixed_name}: {error}"));
    let mut decoded = manifest::decode(
        &fs::read_to_string(fixed.join(MANIFEST_FILE_NAME))
            .unwrap_or_else(|error| panic!("read pinned manifest: {error}")),
    )
    .unwrap_or_else(|error| panic!("decode pinned manifest: {error}"));
    decoded.created_at_unix_ns = created_at;
    let encoded = manifest::encode(&decoded)
        .unwrap_or_else(|error| panic!("encode pinned manifest: {error}"));
    fs::write(fixed.join(MANIFEST_FILE_NAME), encoded)
        .unwrap_or_else(|error| panic!("write pinned manifest: {error}"));
    Some(fixed)
}

/// Rename the just-published snapshot and pad its manifest with whitespace
/// to one byte above the bounded-capture limit. The post-run callback executes
/// after the protocol's self-verification.
fn pin_oversized_fixture(published: &Path, fixed_name: &str, created_at: u128) -> Option<PathBuf> {
    let fixed = pin_fixture(published, fixed_name, created_at)?;
    let manifest_path = fixed.join(MANIFEST_FILE_NAME);
    let raw = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("read pinned manifest before padding: {error}"));
    let decoded = manifest::decode(&raw)
        .unwrap_or_else(|error| panic!("decode pinned manifest before padding: {error}"));
    let consistency = decoded.consistency.as_ref().expect("online repo metadata");
    let main = consistency
        .per_repo
        .get("app/main")
        .expect("fixture contains the main repo");
    let wal_dirs: Vec<_> = decoded
        .wal
        .as_ref()
        .expect("online WAL metadata")
        .dirs
        .iter()
        .filter(|dir| dir.repo == "app/main")
        .map(|dir| format!("{}/", dir.dir))
        .collect();
    let main_files = decoded.files.iter().filter(|entry| {
        entry.path.starts_with(&format!("{}/", main.store_dir))
            || wal_dirs.iter().any(|dir| entry.path.starts_with(dir))
    });
    let expected_bytes: u64 = main_files.clone().map(|entry| entry.size_bytes).sum();
    let expected_files = main_files.count();
    assert!(
        expected_bytes > 0 && expected_files > 0,
        "valid manifest attributes main files"
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&manifest_path)
        .unwrap_or_else(|error| panic!("open pinned manifest for padding: {error}"));
    let current_len = file
        .metadata()
        .unwrap_or_else(|error| panic!("stat pinned manifest: {error}"))
        .len();
    let target_len = super::super::retention_pass::MAX_MANIFEST + 1;
    assert!(
        current_len < target_len,
        "fixture manifest starts below cap"
    );
    let mut remaining = target_len - current_len;
    let padding = [b' '; 8192];
    while remaining > 0 {
        let chunk_len =
            usize::try_from(remaining.min(padding.len() as u64)).expect("padding chunk fits usize");
        file.write_all(&padding[..chunk_len])
            .unwrap_or_else(|error| panic!("pad pinned manifest: {error}"));
        remaining -= chunk_len as u64;
    }
    Some(fixed)
}

fn seed_owned_copy(root: &Path, name: &str, created_at: u128) {
    let dir = root.join(name);
    let wal_dir = "store.shamirwal";
    let wal_file = "store.shamirwal/00000000.wal";
    fs::create_dir_all(dir.join(wal_dir)).expect("WAL directory");
    fs::write(dir.join(wal_file), []).expect("empty WAL segment");
    let manifest = manifest::build_v2(
        created_at,
        vec![ManifestFileEntry {
            path: wal_file.to_owned(),
            sha256: EMPTY_SHA256.to_owned(),
            size_bytes: 0,
        }],
        None,
        None,
        ConsistencySection {
            per_repo: [(
                "repo".to_owned(),
                RepoConsistency {
                    wal_position: 0,
                    store_dir: "store".to_owned(),
                    schema_version: 0,
                },
            )]
            .into(),
            system_store_version: 0,
        },
        WalSection {
            dirs: vec![WalDirEntry {
                repo: "repo".to_owned(),
                dir: wal_dir.to_owned(),
                segments: vec![WalSegmentEntry {
                    seq: 0,
                    file: wal_file.to_owned(),
                    sidecar: None,
                    sha256: EMPTY_SHA256.to_owned(),
                    size: 0,
                }],
            }],
        },
    );
    manifest::write_manifest(&dir, &manifest::encode(&manifest).expect("encode manifest"))
        .expect("write manifest");
    manifest::write_backup_meta(&dir).expect("write backup metadata");
    let sidecar = dir.join(BACKUP_META_FILE_NAME);
    let mut metadata: Value = serde_json::from_slice(&fs::read(&sidecar).expect("read metadata"))
        .expect("decode metadata");
    metadata["scheduled_retention"] = json!({"version": 1, "owner": OWNER});
    fs::write(sidecar, metadata.to_string()).expect("write ownership tag");
    verify_manifest(&dir).expect("retention candidate must verify");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_scheduled_copy_completes_false_without_starting_retention() {
    let guard = FlakeGuard::with_watchdog(
        "failed_scheduled_copy_completes_false_without_starting_retention",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    let source = tempfile::tempdir().expect("source tempdir");
    let dest = tempfile::tempdir().expect("destination tempdir");
    let to = dest.path().join("private");
    create_private_dir(&to).expect("private destination");
    let to = to.canonicalize().expect("canonical destination");
    let root = source.path();
    let deadline = Instant::now() + Duration::from_secs(170);
    let db = tokio::time::timeout(WAIT, async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
            .await
            .expect("init durable engine");
        db.create_db("app").await.expect("create database");
        db.add_repo(
            "app",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
                .add_table(TableConfig::new("rows")),
        )
        .await
        .expect("add durable repo");
        db.flush_all_waiting().await.expect("flush engine");
        db
    })
    .await
    .expect("fixture within deadline");
    guard.mark("durable engine ready");
    let db = Arc::new(db);
    let registry = OnlineBackupRegistry::new(
        Arc::clone(&db),
        root.to_path_buf(),
        tokio::runtime::Handle::current(),
    );
    let copies = ["20200101_000001", "20200101_000002"];
    for (name, created_at) in [(copies[0], 1), (copies[1], 2)] {
        seed_owned_copy(&to, name, created_at);
    }
    // A complete scan would classify this foreign entry as untouched; a scan
    // without a verified fresh copy would also count a retention failure.
    fs::create_dir(to.join("foreign")).expect("foreign directory");
    fs::create_dir(root.join("manifest.json.tmp")).expect("manifest blocker directory");
    fs::write(root.join("manifest.json.tmp/blocker"), b"block").expect("manifest blocker");

    // Nextest gives this test its own process for the process-wide metrics.
    let before = backup_metrics_snapshot();
    assert_eq!(before.retention_untouched, 0);
    let signals = registry
        .start_scheduled(
            to.clone(),
            OWNER.to_owned(),
            BackupRetentionConfig { keep_last: 1 },
        )
        .expect("admit scheduled run");
    guard.mark("scheduled run admitted");
    let completed = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()).min(WAIT),
        signals.outcome,
    )
    .await
    .expect("scheduled completion within deadline")
    .expect("runner explicitly reports failure rather than dropping the sender");
    guard.mark("scheduled run finished");
    assert!(!completed);
    await_finished(signals.finished).await;
    let status = registry.status();
    assert!(!status.running);
    assert_eq!(status.stage, BackupStage::Failed);
    assert_eq!(status.progress.failed_stage, Some(BackupStage::Verifying));
    assert_eq!(status.dest_dir, None);
    let after = backup_metrics_snapshot();
    let verifying_failures = |counts: &[(String, u64)]| {
        counts
            .iter()
            .find(|(stage, _)| stage == "verifying")
            .map_or(0, |(_, count)| *count)
    };
    assert_eq!(
        verifying_failures(&after.failed_total_by_stage),
        verifying_failures(&before.failed_total_by_stage) + 1
    );
    assert_eq!(
        after.retention_deleted_total,
        before.retention_deleted_total
    );
    assert_eq!(after.retention_failed_total, before.retention_failed_total);
    assert_eq!(after.retention_untouched, before.retention_untouched);
    let entries: BTreeSet<_> = fs::read_dir(&to)
        .expect("destination listing")
        .map(|entry| entry.expect("destination entry").file_name())
        .collect();
    // The reader lease marker is provisioned by the scheduled runner after
    // scheduled preflight and before copying; it remains present even when
    // a later verification failure prevents retention from running. Seed
    // `verify_manifest` probes do not create it. It is expected alongside
    // the snapshots, must be a regular file, and retention never lists it.
    let marker_meta = fs::symlink_metadata(to.join(SNAPSHOT_LEASE_FILE_NAME))
        .expect("scheduled runner provisions the lease marker before copying");
    assert!(marker_meta.is_file(), "lease marker must be a regular file");
    assert_eq!(
        entries,
        BTreeSet::from([
            OsString::from(SNAPSHOT_LEASE_FILE_NAME),
            OsString::from(copies[0]),
            OsString::from(copies[1]),
            OsString::from("foreign"),
        ])
    );
    for name in copies {
        verify_manifest(&to.join(name)).expect("existing copy remains verified");
    }
    drop(db);
}

/// A marker-broken successful run with a manifest one byte over the capture
/// limit must refuse provenance and skip the attribution fallback read. The
/// callback runs after protocol verification; its valid, positive attribution
/// fixture proves the old unbounded read would populate the status counters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_manifest_capture_records_no_provenance_or_fallback_attribution() {
    let guard = FlakeGuard::with_watchdog(
        "refused_manifest_capture_records_no_provenance_or_fallback_attribution",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    let source = tempfile::tempdir().expect("source tempdir");
    let dest = tempfile::tempdir().expect("destination tempdir");
    let to = dest.path().join("private");
    create_private_dir(&to).expect("private destination");
    let to = to.canonicalize().expect("canonical destination");
    let root = source.path();
    let db = tokio::time::timeout(WAIT, async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
            .await
            .expect("init durable engine");
        db.create_db("app").await.expect("create database");
        db.add_repo(
            "app",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
                .add_table(TableConfig::new("rows")),
        )
        .await
        .expect("add durable repo");
        db.flush_all_waiting().await.expect("flush engine");
        db
    })
    .await
    .expect("fixture within deadline");
    guard.mark("durable engine ready");
    let db = Arc::new(db);
    let registry = OnlineBackupRegistry::new(
        Arc::clone(&db),
        root.to_path_buf(),
        tokio::runtime::Handle::current(),
    );
    // The obstruction makes pre-copy marker provisioning fail but leaves the
    // destination usable for the scheduled protocol.
    fs::create_dir(to.join(SNAPSHOT_LEASE_FILE_NAME)).expect("marker obstruction");
    let fixed_name = "20200101_000005".to_owned();
    registry.arm_post_run_callback(Box::new(move |published| {
        pin_oversized_fixture(published, &fixed_name, 5)
    }));
    let signals = registry
        .start_scheduled(
            to.clone(),
            OWNER.to_owned(),
            BackupRetentionConfig { keep_last: 1 },
        )
        .expect("admit scheduled run");
    guard.mark("scheduled run admitted");
    let completed = tokio::time::timeout(WAIT, signals.outcome)
        .await
        .expect("scheduled completion within deadline")
        .expect("runner reports completion");
    assert!(completed, "protocol completed successfully");
    guard.mark("scheduled run finished");
    await_finished(signals.finished).await;

    let status = registry.status();
    let snapshot = status.dest_dir.expect("completed snapshot path");
    assert_eq!(snapshot, to.join("20200101_000005"));
    assert_eq!(
        fs::metadata(snapshot.join(MANIFEST_FILE_NAME))
            .expect("padded manifest exists")
            .len(),
        super::super::retention_pass::MAX_MANIFEST + 1
    );
    let manifest_raw = fs::read_to_string(snapshot.join(MANIFEST_FILE_NAME))
        .expect("padded manifest remains readable");
    let manifest_data =
        manifest::decode(&manifest_raw).expect("whitespace-padded manifest remains valid JSON");
    let consistency = manifest_data
        .consistency
        .as_ref()
        .expect("online repo metadata");
    let main = consistency
        .per_repo
        .get("app/main")
        .expect("manifest records the main repo");
    let wal_dirs: Vec<_> = manifest_data
        .wal
        .as_ref()
        .expect("online WAL metadata")
        .dirs
        .iter()
        .filter(|dir| dir.repo == "app/main")
        .map(|dir| format!("{}/", dir.dir))
        .collect();
    let main_files = manifest_data.files.iter().filter(|entry| {
        entry.path.starts_with(&format!("{}/", main.store_dir))
            || wal_dirs.iter().any(|dir| entry.path.starts_with(dir))
    });
    let would_be_bytes: u64 = main_files.clone().map(|entry| entry.size_bytes).sum();
    let would_be_files = main_files.count();
    assert!(would_be_bytes > 0 && would_be_files > 0);
    assert_eq!(registry.pending_adoption_count_for_test(&to, OWNER), 0);
    let main_progress = status
        .progress
        .per_repo
        .iter()
        .find(|progress| progress.repo == "app/main")
        .expect("protocol initialized main repo progress");
    assert_eq!(main_progress.bytes_hashed, 0);
    assert_eq!(main_progress.files_hashed, 0);
    drop(db);
}

/// SAME registry and root: run 1 publishes under a DIRECTORY parked at the
/// marker path — the copy succeeds while retention reports the failure
/// (+1 retention-failure metric, no deletions, no scan so the `untouched`
/// gauge keeps its previous value) and the published copy stays untagged
/// and intact. Run 2 repairs the marker (the obstruction is removed), so
/// provisioning succeeds and the scan adopts EXACTLY run 1's recorded
/// (name, creation time, manifest digest) triple, tags it, and prunes it
/// as the only deletion beyond `keep_last = 1`. Fixed names and a fixed
/// manifest creation time (pinned by the test-only post-run callback via
/// `manifest::decode`/`encode`) make the ordering deterministic — no
/// wall-clock or same-second dependency.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marker_obstruction_counts_retention_failure_and_records_provenance() {
    let guard = FlakeGuard::with_watchdog(
        "marker_obstruction_counts_retention_failure_and_records_provenance",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    let source = tempfile::tempdir().expect("source tempdir");
    let dest = tempfile::tempdir().expect("destination tempdir");
    let to = dest.path().join("private");
    create_private_dir(&to).expect("private destination");
    let to = to.canonicalize().expect("canonical destination");
    let root = source.path();
    let deadline = Instant::now() + Duration::from_secs(170);
    let db = tokio::time::timeout(WAIT, async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
            .await
            .expect("init durable engine");
        db.create_db("app").await.expect("create database");
        db.add_repo(
            "app",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
                .add_table(TableConfig::new("rows")),
        )
        .await
        .expect("add durable repo");
        db.flush_all_waiting().await.expect("flush engine");
        db
    })
    .await
    .expect("fixture within deadline");
    guard.mark("durable engine ready");
    let db = Arc::new(db);
    let registry = OnlineBackupRegistry::new(
        Arc::clone(&db),
        root.to_path_buf(),
        tokio::runtime::Handle::current(),
    );
    // A foreign untagged directory: never adopted, never deleted.
    fs::create_dir(to.join("foreign")).expect("foreign directory");
    // The obstruction: a directory exactly at the marker path. It parks
    // provisioning for run 1 only; run 2 repairs it by removal (below).
    let obstruction = to.join(SNAPSHOT_LEASE_FILE_NAME);
    fs::create_dir(&obstruction).expect("obstruction directory");

    // Nextest gives this test its own process for the process-wide metrics.
    let before = backup_metrics_snapshot();
    assert_eq!(before.retention_untouched, 0);

    // --- Pass 1: copy succeeds, retention reports the marker failure.
    let policy = || BackupRetentionConfig { keep_last: 1 };
    // Test-only fixture surgery: pin run 1's output to a fixed name and a
    // fixed manifest creation time (see `pin_fixture`), so run 2's scan
    // ordering and the recorded provenance are deterministic.
    let run1_name = RUN1_NAME.to_owned();
    registry.arm_post_run_callback(Box::new(move |published| {
        pin_fixture(published, &run1_name, RUN1_CREATED_AT)
    }));
    let signals = registry
        .start_scheduled(to.clone(), OWNER.to_owned(), policy())
        .expect("admit scheduled run");
    guard.mark("run 1 admitted");
    let completed = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()).min(WAIT),
        signals.outcome,
    )
    .await
    .expect("run 1 within deadline")
    .expect("run 1 reports completion");
    assert!(completed, "the scheduled protocol completed successfully");
    // The retention pass (and its metric) runs AFTER the completion signal:
    // the LATE signal is the run's own end (retention + publication).
    await_finished(signals.finished).await;
    let status = registry.status();
    let dir1 = status.dest_dir.clone().expect("run 1 published a copy");
    guard.mark("run 1 finished");
    let after1 = backup_metrics_snapshot();
    assert_eq!(
        after1.retention_failed_total,
        before.retention_failed_total + 1
    );
    assert_eq!(
        after1.retention_deleted_total,
        before.retention_deleted_total
    );
    assert_eq!(after1.retention_untouched, before.retention_untouched);
    assert!(dir1.is_dir(), "run 1's output is visible at its fixed name");
    assert_eq!(dir1, to.join(RUN1_NAME), "the callback pinned the name");
    let sidecar1: Value =
        serde_json::from_slice(&fs::read(dir1.join(BACKUP_META_FILE_NAME)).expect("read sidecar"))
            .expect("decode sidecar");
    assert!(
        sidecar1.get("scheduled_retention").is_none(),
        "this run publishes without a tag; a later pass can adopt it only with the recorded manifest provenance"
    );
    assert!(to.join("foreign").is_dir());

    // --- Repair: remove the obstruction, pin run 2's output to its own
    // fixed name/time, then run 2 provisions the marker successfully and
    // the scan adopts run 1's recorded provenance.
    fs::remove_dir(&obstruction).expect("marker path repaired");
    let run2_name = RUN2_NAME.to_owned();
    registry.arm_post_run_callback(Box::new(move |published| {
        pin_fixture(published, &run2_name, RUN2_CREATED_AT)
    }));
    let signals2 = registry
        .start_scheduled(to.clone(), OWNER.to_owned(), policy())
        .expect("admit scheduled run 2");
    guard.mark("run 2 admitted");
    let completed2 = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()).min(WAIT),
        signals2.outcome,
    )
    .await
    .expect("run 2 within deadline")
    .expect("run 2 reports completion");
    assert!(completed2, "run 2's copy must succeed");
    guard.mark("run 2 finished");
    await_finished(signals2.finished).await;
    let status2 = registry.status();
    assert!(!status2.running);
    let dir2 = status2.dest_dir.clone().expect("run 2 published a copy");
    assert!(dir2.is_dir());
    assert_eq!(
        dir2,
        to.join(RUN2_NAME),
        "run 2's copy is at its fixed name"
    );
    assert_ne!(dir2, dir1, "the two runs pinned distinct names");
    let after2 = backup_metrics_snapshot();
    // Run 1's marker failure is the only retention failure; run 2
    // provisioned fine.
    assert_eq!(
        after2.retention_failed_total,
        before.retention_failed_total + 1
    );
    // Exactly ONE deletion: run 1's adopted copy, the only entry beyond
    // `keep_last = 1` (foreign is never a candidate).
    assert_eq!(
        after2.retention_deleted_total,
        before.retention_deleted_total + 1
    );
    // The foreign untagged directory was counted untouched by run 2's
    // complete scan (run 1 ran no scan under the broken marker).
    assert_eq!(after2.retention_untouched, before.retention_untouched + 1);
    // Run 1's real, previously untagged snapshot was adopted, tagged and
    // then deleted by run 2's prune — exactly once, exactly by name.
    assert!(!dir1.exists(), "run 1's adopted copy was pruned by run 2");
    let sidecar2: Value =
        serde_json::from_slice(&fs::read(dir2.join(BACKUP_META_FILE_NAME)).expect("read sidecar"))
            .expect("decode sidecar");
    assert_eq!(
        sidecar2["scheduled_retention"]["owner"],
        json!(OWNER),
        "run 2's fresh copy was tagged by the provisioned marker"
    );
    let entries2: BTreeSet<_> = fs::read_dir(&to)
        .expect("destination listing")
        .map(|entry| entry.expect("destination entry").file_name())
        .collect();
    assert_eq!(
        entries2,
        BTreeSet::from([
            OsString::from(SNAPSHOT_LEASE_FILE_NAME),
            OsString::from("foreign"),
            OsString::from(RUN2_NAME),
        ]),
        "foreign survives; run 1's copy is exactly deleted"
    );
    verify_manifest(&dir2).expect("run 2's copy remains verified");
    drop(db);
}

/// A failed SCHEDULED run renders the stage word plus a bounded reason token in
/// `status().last_error` and in ONE log line, with neither the destination path
/// nor any path separator. The destination root carries a canary component, and
/// a directory planted at the source's `manifest.json.tmp` is copied into the
/// snapshot, so the manifest write fails at `Verifying` with an error whose
/// `Display` names that destination path.
///
/// Mutant: publishing the raw error (or its `Display` chain) leaks the canary,
/// and not logging the final scheduled outcome leaves no matching record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_failure_renders_stage_and_reason_without_paths() {
    let guard = FlakeGuard::with_watchdog(
        "scheduled_failure_renders_stage_and_reason_without_paths",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    let source = tempfile::tempdir().expect("source tempdir");
    let dest = tempfile::tempdir().expect("destination tempdir");
    let canary = format!("CANARY-{}", std::process::id());
    let to = dest.path().join(&canary);
    create_private_dir(&to).expect("private destination");
    let to = to.canonicalize().expect("canonical destination");
    let root = source.path();
    let deadline = Instant::now() + Duration::from_secs(170);
    let db = tokio::time::timeout(WAIT, async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
            .await
            .expect("init durable engine");
        db.create_db("app").await.expect("create database");
        db.add_repo(
            "app",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
                .add_table(TableConfig::new("rows")),
        )
        .await
        .expect("add durable repo");
        db.flush_all_waiting().await.expect("flush engine");
        db
    })
    .await
    .expect("fixture within deadline");
    guard.mark("durable engine ready");
    let db = Arc::new(db);
    let registry = OnlineBackupRegistry::new(
        Arc::clone(&db),
        root.to_path_buf(),
        tokio::runtime::Handle::current(),
    );
    // The residual pass copies this directory into the snapshot; writing the
    // snapshot manifest then fails (a file cannot replace a directory).
    fs::create_dir(root.join("manifest.json.tmp")).expect("manifest blocker directory");
    fs::write(root.join("manifest.json.tmp/blocker"), b"block").expect("manifest blocker");

    let signals = registry
        .start_scheduled(
            to.clone(),
            OWNER.to_owned(),
            BackupRetentionConfig { keep_last: 1 },
        )
        .expect("admit scheduled run");
    guard.mark("scheduled run admitted");
    let completed = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()).min(WAIT),
        signals.outcome,
    )
    .await
    .expect("scheduled completion within deadline")
    .expect("runner explicitly reports failure rather than dropping the sender");
    assert!(!completed, "the forced copy failure must be reported");
    await_finished(signals.finished).await;
    guard.mark("scheduled run finished");

    let last_error = registry
        .status()
        .last_error
        .expect("the failed scheduled run records a bounded error");
    let (stage, reason) = last_error
        .split_once(": ")
        .unwrap_or_else(|| panic!("expected `stage: reason`, got {last_error}"));
    assert_eq!(stage, "verifying", "stage word in {last_error}");
    assert!(
        reason.contains("io-") && !reason.contains(' '),
        "the reason must be a bounded io token, got {last_error}"
    );
    assert!(
        !last_error.contains(&canary),
        "the destination path must never reach the status: {last_error}"
    );
    assert!(
        !last_error.contains('/') && !last_error.contains('\\'),
        "no path separator may reach the status: {last_error}"
    );

    // The final failed scheduled outcome is logged ONCE with stage + reason.
    let logged = shamir_test_diag::ring::shared_ring()
        .snapshot()
        .into_iter()
        .find(|record| {
            record.message.contains("scheduled backup run failed")
                && record.message.contains(stage)
                && record.message.contains(reason)
        })
        .unwrap_or_else(|| panic!("no path-free scheduled-failure log line was captured"));
    assert!(
        !logged.message.contains(&canary),
        "the log line must not carry the destination path: {}",
        logged.message
    );
    drop(db);
}

/// One bounded condition-variable gate `(set, wake)`.
type Gate = (Mutex<bool>, Condvar);

/// Wait (bounded) until `gate` was opened on another thread.
fn wait_for_gate(gate: &Gate, what: &str) {
    let (lock, condvar) = gate;
    let deadline = Instant::now() + WAIT;
    let mut opened = lock.lock().expect("gate lock");
    while !*opened {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "{what} must happen within the bound");
        let (next, _) = condvar.wait_timeout(opened, remaining).expect("gate wait");
        opened = next;
    }
}

/// Open `gate` and wake every waiter.
fn open_gate(gate: &Gate) {
    let (lock, condvar) = gate;
    *lock.lock().expect("gate lock") = true;
    condvar.notify_all();
}

/// While a scheduled run's retention is PARKED through the one-shot pause
/// seam, the record is still `running` (admission is `record.running` through
/// retention), so BOTH admission paths refuse with `AlreadyRunning`. Releasing
/// the park with a PANIC makes the registry's `catch_unwind` account exactly
/// ONE retention failure: the copy outcome stays the single success it already
/// was (one `completed` increment, streak 0, no failed stage), and the run
/// still publishes `running = false` and sends its LATE `finished` signal.
///
/// Mutants: publishing `running = false` before retention (or dropping the
/// retention panic) lets a second start in; losing the panic's accounting or
/// counting it as a copy failure changes the counters below.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_retention_refuses_admission_and_a_panic_is_one_retention_failure() {
    let guard = FlakeGuard::with_watchdog(
        "parked_retention_refuses_admission_and_a_panic_is_one_retention_failure",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    let source = tempfile::tempdir().expect("source tempdir");
    let dest = tempfile::tempdir().expect("destination tempdir");
    let to = dest.path().join("private");
    create_private_dir(&to).expect("private destination");
    let to = to.canonicalize().expect("canonical destination");
    let manual = dest.path().join("manual");
    create_private_dir(&manual).expect("private manual destination");
    let manual = manual.canonicalize().expect("canonical manual destination");
    let root = source.path();
    let db = tokio::time::timeout(WAIT, async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
            .await
            .expect("init durable engine");
        db.create_db("app").await.expect("create database");
        db.add_repo(
            "app",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
                .add_table(TableConfig::new("rows")),
        )
        .await
        .expect("add durable repo");
        db.flush_all_waiting().await.expect("flush engine");
        db
    })
    .await
    .expect("fixture within deadline");
    guard.mark("durable engine ready");
    let db = Arc::new(db);
    let registry = OnlineBackupRegistry::new(
        Arc::clone(&db),
        root.to_path_buf(),
        tokio::runtime::Handle::current(),
    );

    // Nextest gives this test its own process for the process-wide metrics.
    let before = backup_metrics_snapshot();
    assert_eq!(before.completed_total, 0);
    assert_eq!(before.scheduled_failure_streak, 0);

    // The runner thread parks INSIDE retention, signals the park, then blocks
    // until the test releases it — and the release PANICS, still inside the
    // registry's `catch_unwind` around the retention pass.
    let parked_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let parked_at = Arc::clone(&parked_gate);
    let release_after = Arc::clone(&release_gate);
    registry.arm_post_run_callback(Box::new(move |_published: &Path| -> Option<PathBuf> {
        // Runs ON THE RUNNER THREAD, after the copy and before retention: arm
        // the one-shot pause there (the pause seam is thread-local).
        let parked = Arc::clone(&parked_at);
        let release = Arc::clone(&release_after);
        let pause = arm_scheduled_retention_pause(move || {
            open_gate(&parked);
            wait_for_gate(&release, "the test's release of the parked retention");
            panic!("planted retention panic (the registry's catch_unwind must account it)");
        });
        // The pause fires in THIS run's retention pass; the zero-sized guard
        // would only clear an unfired install.
        std::mem::forget(pause);
        None
    }));
    let signals = registry
        .start_scheduled(
            to.clone(),
            OWNER.to_owned(),
            BackupRetentionConfig { keep_last: 1 },
        )
        .expect("admit scheduled run");
    guard.mark("scheduled run admitted");
    wait_for_gate(&parked_gate, "the retention park");
    guard.mark("retention parked");

    // Admission is `record.running`, still `true` through retention.
    assert!(
        matches!(
            registry.start(manual),
            Err(StartBackupError::AlreadyRunning)
        ),
        "a manual start must be refused while retention is parked"
    );
    assert!(
        matches!(
            registry.start_scheduled(
                to.clone(),
                OWNER.to_owned(),
                BackupRetentionConfig { keep_last: 1 },
            ),
            Err(StartBackupError::AlreadyRunning)
        ),
        "a scheduled start must be refused while retention is parked"
    );
    guard.mark("both admissions refused while parked");

    open_gate(&release_gate);
    let completed = tokio::time::timeout(WAIT, signals.outcome)
        .await
        .expect("the copy outcome is published before retention")
        .expect("the runner must report the copy outcome");
    assert!(completed, "the copy itself succeeded");
    await_finished(signals.finished).await;
    guard.mark("run finished");

    let status = registry.status();
    assert!(!status.running, "the final publication releases admission");
    assert_eq!(status.stage, BackupStage::Done);
    assert!(
        status.last_error.is_none(),
        "a retention panic is never a copy error: {:?}",
        status.last_error
    );
    let after = backup_metrics_snapshot();
    assert_eq!(
        after.completed_total,
        before.completed_total + 1,
        "exactly ONE successful copy outcome"
    );
    assert_eq!(
        after.scheduled_failure_streak, 0,
        "the success keeps the failure streak at zero"
    );
    assert_eq!(
        after.failed_total_by_stage, before.failed_total_by_stage,
        "the retention panic must not be counted as a copy failure"
    );
    assert_eq!(
        after.retention_failed_total,
        before.retention_failed_total + 1,
        "exactly ONE retention failure"
    );
    assert_eq!(
        after.retention_deleted_total, before.retention_deleted_total,
        "a panicked pass deletes nothing"
    );
    drop(db);
}
