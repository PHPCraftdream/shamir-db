use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
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
use crate::sync::durability::backup::{verify_manifest, ManifestFileEntry};

use super::super::super::metrics::backup_metrics_snapshot;
use super::super::super::OnlineBackupRegistry;

const OWNER: &str = "registry-test-owner";
const WAIT: Duration = Duration::from_secs(120);
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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
    assert_eq!(
        entries,
        BTreeSet::from([copies[0], copies[1], "foreign"].map(OsString::from))
    );
    for name in copies {
        verify_manifest(&to.join(name)).expect("existing copy remains verified");
    }
    drop(db);
}
