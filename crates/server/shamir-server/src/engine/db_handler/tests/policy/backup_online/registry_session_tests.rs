//! `OnlineBackupRegistry` session tests over a real durable engine (R2a
//! step 3a item 6, design `online-backup.md` §4.4/§4.7, D9, R4).
//!
//! The registry drives the REAL `EngineBackupPort` over a real
//! `ShamirDb` (persistent fjall system store, one file-backed repo). Runs
//! complete successfully: the system-store consistency point comes from
//! the facade `ShamirDb::capture_system_store_point()`. These tests
//! exercise: the typed already-running refusal (D9), status surviving
//! until the next start (§4.7), cancel flipping the flag and ending the
//! run with the destination removed (R4), and the engine being left fully
//! open (the next run re-passes catalogue/pins/capture, proving the
//! pins/seals of the previous run were released).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::backup_online::{OnlineBackupRegistry, StartBackupError};
use crate::sync::durability::backup::online::manifest;

const WAIT: Duration = Duration::from_secs(60);

/// One durable engine (see module doc) plus the registry over it. The data dir
/// passed to the registry is the system store's parent — the same root the
/// engine catalogues repo store paths against.
async fn fixture(root: &Path) -> (Arc<ShamirDb>, OnlineBackupRegistry) {
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
    let db = Arc::new(db);
    let registry = OnlineBackupRegistry::new(
        Arc::clone(&db),
        root.to_path_buf(),
        tokio::runtime::Handle::current(),
    );
    (db, registry)
}

/// Bounded wait for a background-runner condition (the runner thread has
/// no completion channel, so the test polls the status snapshot under a
/// hard deadline — never an unbounded sleep).
fn wait_until(mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::park_timeout(Duration::from_millis(2));
    }
    check()
}

/// Cancel a still-running run and wait (bounded) for the runner to finish.
fn cancel_and_wait(registry: &OnlineBackupRegistry) {
    assert!(registry.cancel(), "run must be active to cancel");
    assert!(
        wait_until(|| !registry.status().running),
        "runner must finish after cancel"
    );
}

/// Destination root outside the data dir (a sibling tempdir).
fn dest_root() -> PathBuf {
    tempfile::tempdir().expect("tempdir").keep()
}

/// A second `start` while a run is in flight is refused synchronously with
/// the typed `AlreadyRunning` error (design D9), and after the first run
/// ends the gate is free again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_start_while_running_is_refused_with_typed_already_running() {
    let root = tempfile::tempdir().expect("tempdir").keep();
    let (_db, registry) = fixture(&root).await;
    let to = dest_root();

    registry.start(to.clone()).expect("first start");
    assert!(registry.status().running, "record must be running");

    // The first run is in flight; the synchronous gate must refuse. Each
    // `Ok` would mean the previous run had already ended, so the retry
    // simply re-arms a run and re-attempts — the next call is issued
    // microseconds into a fresh run, well inside its in-flight window.
    let mut refused = false;
    for _ in 0..32 {
        match registry.start(to.clone()) {
            Err(StartBackupError::AlreadyRunning) => {
                refused = true;
                break;
            }
            Err(other) => panic!("unexpected start error: {other}"),
            Ok(_) => continue,
        }
    }
    assert!(
        refused,
        "a concurrent second start must be refused with AlreadyRunning"
    );
    if registry.status().running {
        cancel_and_wait(&registry);
    }
    let _ = std::fs::remove_dir_all(&to);
    let _ = std::fs::remove_dir_all(&root);
}

/// Invalid destinations are refused before any run is registered, with the
/// typed `InvalidDestination` variant: relative paths and anything inside
/// (or equal to) the source data dir.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_destinations_are_refused_with_typed_error() {
    let root = tempfile::tempdir().expect("tempdir").keep();
    let (_db, registry) = fixture(&root).await;

    for (alias, to) in [
        ("relative", PathBuf::from("backup-out")),
        ("inside-data-dir", root.join("backup-out")),
        ("data-dir-itself", root.clone()),
    ] {
        match registry.start(to) {
            Err(StartBackupError::InvalidDestination(reason)) => {
                assert!(!reason.is_empty(), "{alias} must carry a reason");
            }
            other => panic!("{alias} must be refused as invalid, got: {other:?}"),
        }
    }
    assert!(!registry.status().running, "no run may be registered");
    assert_eq!(registry.status().backup_id, "", "no backup id assigned");
    let _ = std::fs::remove_dir_all(&root);
}

/// `cancel` flips the flag while a run is active, the run ends with the
/// destination root empty (R4) and a typed cancelled failure recorded, and
/// the engine is fully open afterwards (the next run re-passes
/// catalogue/pins/capture, reaching the same fail-closed error).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_flips_flag_ends_run_and_leaves_engine_open() {
    let root = tempfile::tempdir().expect("tempdir").keep();
    let (_db, registry) = fixture(&root).await;
    let to = dest_root();

    registry.start(to.clone()).expect("start");
    assert!(registry.status().running);
    assert!(registry.cancel(), "cancel must report an active run");
    assert!(
        registry.status().cancel_requested,
        "cancel flag must be observable in the status"
    );
    assert!(
        wait_until(|| !registry.status().running),
        "run must end after cancel"
    );
    assert_eq!(
        std::fs::read_dir(&to).expect("to dir").count(),
        0,
        "cancelled run must leave the destination root empty"
    );
    let snapshot = registry.status();
    assert!(
        snapshot
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("cancelled"),
        "cancellation must be recorded as the last error: {:?}",
        snapshot.last_error
    );
    assert_eq!(snapshot.dest_dir, None);
    // Whether the runner got as far as the catalogue before observing the
    // flag is a benign race; both arms record a cancelled end.
    assert!(
        snapshot.progress.per_repo.len() <= 1,
        "at most the one captured repo may appear in the progress record"
    );

    // The engine is fully open: a fresh run passes catalogue/pins/capture
    // again, proving the previous run's pins and
    // seals were released and the permit is free.
    let (id, destination) = registry.start(to.clone()).expect("restart after cancel");
    assert!(id.starts_with("ob-"), "backup id shape: {id}");
    assert_eq!(destination, to);
    cancel_and_wait(&registry);
    let after = registry.status();
    assert_eq!(after.dest_dir, None, "cancelled run has no snapshot dir");
    let _ = std::fs::remove_dir_all(&to);
    let _ = std::fs::remove_dir_all(&root);
}

/// The finished-run status survives until the next `start` (§4.7): the
/// successful run's stage, snapshot dir, per-repo progress and the manifest
/// (with the facade-derived system-store version) all remain readable, and
/// a new start replaces the record with a fresh id and destination.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_survives_until_next_start_and_shows_stage_progress() {
    let root = tempfile::tempdir().expect("tempdir").keep();
    let (_db, registry) = fixture(&root).await;
    let to = dest_root();

    registry.start(to.clone()).expect("start");
    assert!(
        wait_until(|| !registry.status().running),
        "run must complete on its own"
    );

    let snapshot = registry.status();
    assert!(!snapshot.running);
    assert!(
        snapshot.dest_dir.is_some(),
        "a successful run records the snapshot dir"
    );
    assert!(
        snapshot.last_error.is_none(),
        "success records no error: {:?}",
        snapshot.last_error
    );
    assert_eq!(
        format!("{:?}", snapshot.stage),
        "Done",
        "the protocol's final stage must surface in the status"
    );
    let repo_progress = &snapshot.progress.per_repo;
    assert_eq!(repo_progress.len(), 1, "one repo in the progress record");
    assert_eq!(repo_progress[0].repo, "app/main");
    assert!(
        repo_progress[0].bytes_hashed > 0,
        "the run's hashed bytes must be visible after completion"
    );

    // The manifest carries the real system-store version from
    // `ShamirDb::capture_system_store_point` (> 0: the fixture wrote
    // catalogue rows).
    let manifest_path = snapshot
        .dest_dir
        .as_ref()
        .expect("dest dir")
        .join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).expect("manifest bytes");
    let decoded = manifest::decode(&raw).expect("manifest decodes");
    let consistency = decoded
        .consistency
        .as_ref()
        .expect("v2 manifest carries a consistency section");
    assert!(
        consistency.system_store_version > 0,
        "system_store_version must reflect the catalogue write horizon"
    );

    // Idle registry: cancel is a no-op and reports false.
    assert!(!registry.cancel());

    // The next start resets the record: fresh id, new destination, running.
    let other = dest_root();
    let first_id = snapshot.backup_id.clone();
    let (second_id, destination) = registry.start(other.clone()).expect("second start");
    assert!(second_id.starts_with("ob-"));
    assert_ne!(second_id, first_id, "each start gets a fresh backup id");
    let fresh = registry.status();
    assert!(fresh.running);
    assert_eq!(fresh.destination, other);
    assert_eq!(destination, other);
    assert_eq!(fresh.backup_id, second_id);
    assert!(
        fresh.last_error.is_none(),
        "new run starts with a clean record"
    );
    cancel_and_wait(&registry);
    let _ = std::fs::remove_dir_all(snapshot.dest_dir.as_ref().expect("dest dir"));
    let _ = std::fs::remove_dir_all(&other);
    let _ = std::fs::remove_dir_all(&root);
}
