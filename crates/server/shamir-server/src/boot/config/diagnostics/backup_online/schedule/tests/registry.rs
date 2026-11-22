use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_private_dir::create_private_dir;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};

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
use super::super::super::OnlineBackupRegistry;

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
    let completion = registry
        .start_scheduled(
            to.clone(),
            OWNER.to_owned(),
            BackupRetentionConfig {
                keep_last: 1,
                max_age_days: None,
            },
        )
        .expect("admit scheduled run");
    guard.mark("scheduled run admitted");
    let completed = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()).min(WAIT),
        completion,
    )
    .await
    .expect("scheduled completion within deadline")
    .expect("runner explicitly reports failure rather than dropping the sender");
    guard.mark("scheduled run finished");
    assert!(!completed);
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

/// Marker-provisioning defect, real registry, TWO successful runs on the
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
    let policy = || BackupRetentionConfig {
        keep_last: 1,
        max_age_days: None,
    };
    // Test-only fixture surgery: pin run 1's output to a fixed name and a
    // fixed manifest creation time (see `pin_fixture`), so run 2's scan
    // ordering and the recorded provenance are deterministic.
    let run1_name = RUN1_NAME.to_owned();
    registry.arm_post_run_callback(Box::new(move |published| {
        pin_fixture(published, &run1_name, RUN1_CREATED_AT)
    }));
    let completion = registry
        .start_scheduled(to.clone(), OWNER.to_owned(), policy())
        .expect("admit scheduled run");
    guard.mark("run 1 admitted");
    let completed = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()).min(WAIT),
        completion,
    )
    .await
    .expect("run 1 within deadline")
    .expect("run 1 reports completion");
    assert!(completed, "the copy itself must succeed");
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
        "a copy published without a marker must stay untagged"
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
    let completion2 = registry
        .start_scheduled(to.clone(), OWNER.to_owned(), policy())
        .expect("admit scheduled run 2");
    guard.mark("run 2 admitted");
    let completed2 = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()).min(WAIT),
        completion2,
    )
    .await
    .expect("run 2 within deadline")
    .expect("run 2 reports completion");
    assert!(completed2, "run 2's copy must succeed");
    guard.mark("run 2 finished");
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
