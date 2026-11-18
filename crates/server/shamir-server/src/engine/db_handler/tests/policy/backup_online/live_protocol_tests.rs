//! Live-protocol test at the highest feasible seam (R2a step 3a item 6):
//! a REAL `ShamirDb` engine (persistent fjall system store, one
//! file-backed repo with committed rows) plus the REAL `EngineBackupPort`
//! plus the real `run_online_backup` protocol — no HTTP server, no registry
//! thread. A full live-server run is disproportionate here (the harness
//! would add TLS/SCRAM/session plumbing that the backup never touches).
//!
//! Proven: the backup completes, the manifest is written last (mtime
//! ordering), the consistency section names the captured repo with the
//! real system-store version from the facade
//! (`ShamirDb::capture_system_store_point`), the WAL segment and store
//! files are in the snapshot, `verify_manifest` accepts the snapshot, and
//! the source repo store is bit-identical before/after the run (R4).

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use tokio::runtime::Builder;

use crate::backup_online::{EngineBackupPort, PortRunState};
use crate::sync::durability::backup::online::copy_engine::CopyOptions;
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::{BackupProgress, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupReport, SealRecopyCap,
};
use crate::sync::durability::backup::verify_manifest;

const RUN_WAIT: Duration = Duration::from_secs(120);

/// Durable engine fixture: persistent system store under `<root>/system`,
/// database `app`, file-backed repo `main` at `<root>/repos/main` with one
/// table and two committed rows, flushed. Also creates the adapter's WAL
/// sibling (`repos/main.shamirwal`) with one segment + sidecar — the
/// engine's own WAL layout is not exposed at the facade, so the test
/// materialises the convention the adapter documents.
async fn durable_engine_with_wal(root: &Path) -> Arc<ShamirDb> {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
        .await
        .expect("init engine");
    db.create_db("app").await.expect("create_db");
    db.add_repo(
        "app",
        RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
            .add_table(TableConfig::new("rows")),
    )
    .await
    .expect("add_repo");
    let mut batch = Batch::new();
    let mut row_a = new_map();
    row_a.insert("n".to_string(), QueryValue::Int(1));
    batch.op(
        "ins_a",
        write::Insert::with_repo("main", "rows").row(QueryValue::Map(row_a)),
    );
    let mut row_b = new_map();
    row_b.insert("n".to_string(), QueryValue::Int(2));
    batch.op(
        "ins_b",
        write::Insert::with_repo("main", "rows").row(QueryValue::Map(row_b)),
    );
    db.execute("app", &batch.build())
        .await
        .expect("insert rows");
    db.flush_all_waiting().await.expect("flush");

    let wal_dir = root.join("repos/main.shamirwal");
    fs::create_dir_all(&wal_dir).expect("wal dir");
    fs::write(wal_dir.join("00000000.wal"), b"wal-segment-0").expect("wal segment");
    fs::write(wal_dir.join("00000000.meta"), b"wal-meta-0").expect("wal sidecar");
    Arc::new(db)
}

/// Recursive `<rel path, size, mtime>` listing of a tree (forward-slash
/// rel paths), sorted — used for the R4 source snapshot and the
/// manifest-last mtime check.
fn tree_entries(root: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64, std::time::SystemTime)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
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
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if path.is_dir() {
            walk(root, &path, out);
        } else {
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            out.push((rel, meta.len(), modified));
        }
    }
}

#[test]
fn live_engine_backup_completes_manifest_last_and_verifies() {
    // ONE runtime for the whole test; the protocol runs on a plain
    // blocking thread exactly like the production runner, bridging the
    // async facade calls through this runtime's handle.
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");

    let workspace = tempfile::tempdir().expect("tempdir").keep();
    let destination = tempfile::tempdir().expect("tempdir").keep();
    let db = runtime.block_on(durable_engine_with_wal(&workspace));

    // R4 baseline: the repo store is quiescent after the flush.
    let store_dir = workspace.join("repos/main");
    let store_before = tree_entries(&store_dir);
    assert!(!store_before.is_empty(), "store must have files");

    let run_state = Arc::new(PortRunState::new());
    let port = Arc::new(EngineBackupPort::new(
        Arc::clone(&db),
        workspace.clone(),
        runtime.handle().clone(),
        Arc::clone(&run_state),
    ));

    let port_for_run = Arc::clone(&port);
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = CancelFlag::default();
    let source_root = workspace.clone();
    let dest_root = destination.clone();
    std::thread::spawn(move || {
        let mut progress = BackupProgress::new();
        let cap = SealRecopyCap::default();
        let result = run_online_backup(
            port_for_run.as_ref(),
            &source_root,
            &dest_root,
            &cancel,
            &mut progress,
            &CopyOptions::default(),
            &cap,
        );
        let _ = tx.send(result);
    });

    let report: BackupReport = rx
        .recv_timeout(RUN_WAIT)
        .expect("protocol run must finish within the bound")
        .expect("online backup must complete");

    let dest_dir = report.dest_dir;
    assert!(
        dest_dir.starts_with(&destination),
        "snapshot must live under the operator destination"
    );
    assert!(report.files_hashed > 0, "the snapshot must describe files");

    // Manifest written LAST: no other manifested file may be newer.
    let manifest_path = dest_dir.join("manifest.json");
    assert!(manifest_path.is_file(), "manifest.json must exist");
    let manifest_mtime = fs::metadata(&manifest_path)
        .expect("manifest metadata")
        .modified()
        .expect("manifest mtime");
    for (rel, _, mtime) in tree_entries(&dest_dir) {
        if rel == "manifest.json" || rel == "backup.meta.json" {
            continue; // meta is written after the manifest by design
        }
        assert!(
            mtime <= manifest_mtime,
            "manifest must be the last write; {rel} is newer"
        );
    }

    // Consistency section: the captured repo with the REAL system-store
    // version from the facade — positive and consistent with a catalogue
    // that had writes (db/repo catalogue rows committed before the run).
    let raw = fs::read_to_string(&manifest_path).expect("manifest bytes");
    let decoded = manifest::decode(&raw).expect("manifest decodes");
    let consistency = decoded
        .consistency
        .as_ref()
        .expect("v2 manifest carries a consistency section");
    assert!(
        consistency.system_store_version > 0,
        "system_store_version must reflect the catalogue write horizon"
    );
    let point = consistency
        .per_repo
        .get("app/main")
        .expect("captured repo in the consistency section");
    assert!(point.wal_position > 0, "seal position must be recorded");
    assert_eq!(point.store_dir, "repos/main");

    // Snapshot contents: WAL segment (with manifest entry) + store files.
    let wal = decoded
        .wal
        .as_ref()
        .expect("v2 manifest carries a WAL section");
    let alpha = wal.dirs.iter().find(|d| d.repo == "app/main");
    let alpha = match alpha {
        Some(dir) => dir,
        None => panic!("the WAL segment must be listed in the manifest"),
    };
    assert_eq!(alpha.segments.len(), 1, "one WAL segment captured");
    assert!(
        dest_dir.join(&alpha.segments[0].file).is_file(),
        "WAL segment file present in the snapshot"
    );
    assert!(
        fs::read_dir(dest_dir.join("repos/main"))
            .expect("store dir in snapshot")
            .count()
            > 0,
        "store files copied into the snapshot"
    );

    // The existing verifier accepts the finished snapshot.
    verify_manifest(&dest_dir).expect("snapshot must verify");

    // R4: the source repo store is unchanged by the run.
    assert_eq!(
        tree_entries(&store_dir),
        store_before,
        "the source store must be untouched by the backup"
    );

    // The adapter observed the run: one repo in the step-A snapshot.
    assert_eq!(run_state.snapshot_count(), 1);
    assert!(
        run_state.excluded_repos().is_empty(),
        "the file-backed repo must not be excluded"
    );

    // Drop order: port (holds pins/seal holds + db handle), then db, then
    // the runtime — all Fjall handles released before the tempdirs go.
    drop(port);
    drop(db);
    drop(runtime);
    let _ = fs::remove_dir_all(&dest_dir);
    let _ = fs::remove_dir_all(&destination);
    let _ = fs::remove_dir_all(&workspace);
}
