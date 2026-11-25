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
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::backup_online::metrics::backup_metrics_snapshot;
use crate::backup_online::{
    EngineBackupPort, OnlineBackupRegistry, PortRunState, StartBackupError,
};
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::BackupStage;
use crate::sync::durability::backup::online::wal_capture::{OnlineBackupPort, PortError};
use crate::sync::durability::backup::verify_manifest;

/// Per-operation hang detector: allow for a runner up to ten times slower
/// than the 12 s baseline, subject to the remaining overall budget. Every
/// await, poll and blocking `recv_timeout` here uses at least this (CI
/// evidence on Windows showed over 24 s elapsed per test against the
/// previous tighter bounds).
const WAIT: Duration = Duration::from_secs(120);
/// Overall cap for a multi-phase test: every await is additionally capped
/// by the remaining time to this deadline, so a stalled phase cannot push
/// the test past the 180 s nextest per-test kill.
const OVERALL: Duration = Duration::from_secs(170);

/// Remaining time to an overall deadline, never below zero and never above
/// `WAIT`: each await may take up to `WAIT`, shortened if needed by the
/// overall limit — `WAIT` is an upper bound, never a minimum.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now()).min(WAIT)
}

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
fn wait_until(mut check: impl FnMut() -> bool, outer: Instant) -> bool {
    let deadline = Instant::now() + remaining(outer);
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::park_timeout(Duration::from_millis(2));
    }
    check()
}

/// Cancel a still-running run and wait (bounded) for the runner to finish.
fn cancel_and_wait(registry: &OnlineBackupRegistry, deadline: Instant) {
    assert!(registry.cancel(), "run must be active to cancel");
    assert!(
        wait_until(|| !registry.status().running, deadline,),
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
    let deadline = Instant::now() + OVERALL;
    let (_db, registry) = tokio::time::timeout(remaining(deadline), fixture(&root))
        .await
        .expect("fixture within the bound");
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
        cancel_and_wait(&registry, deadline);
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
    let deadline = Instant::now() + OVERALL;
    let (_db, registry) = tokio::time::timeout(remaining(deadline), fixture(&root))
        .await
        .expect("fixture within the bound");

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
    let deadline = Instant::now() + OVERALL;
    let (_db, registry) = tokio::time::timeout(remaining(deadline), fixture(&root))
        .await
        .expect("fixture within the bound");
    let to = dest_root();

    registry.start(to.clone()).expect("start");
    assert!(registry.status().running);
    assert!(registry.cancel(), "cancel must report an active run");
    assert!(
        registry.status().cancel_requested,
        "cancel flag must be observable in the status"
    );
    assert!(
        wait_until(|| !registry.status().running, deadline,),
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
    cancel_and_wait(&registry, deadline);
    let after = registry.status();
    assert_eq!(after.dest_dir, None, "cancelled run has no snapshot dir");
    let _ = std::fs::remove_dir_all(&to);
    let _ = std::fs::remove_dir_all(&root);
}

/// A deterministic asynchronous failure (not a synchronous `start`
/// refusal): a `manifest.json.tmp/` directory seeded at the top level of
/// the data root is copied verbatim by the protocol's residual pass (only
/// `cache` is skipped at the top level) and then blocks the tmp-then-rename
/// manifest write (step F) — the `port_fault_injection_tests`
/// `manifest_write_conflict` seam over the REAL engine. The run ends with
/// the failure recorded, no snapshot dir and the destination cleaned; the
/// engine stays fully open and a second `start` (after the blocker is
/// removed) succeeds and completes to `Done`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_failure_then_second_start_succeeds_and_engine_stays_usable() {
    // Failure-only diagnostics: the watchdog dumps the phase timeline
    // after 150 s, below the 180 s nextest per-test kill, so a hang
    // reports where it stuck instead of being silently reaped.
    let guard = FlakeGuard::with_watchdog(
        "async_failure_then_second_start_succeeds_and_engine_stays_usable",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    // RAII tempdirs: kept alive to the end of the test, removed on drop —
    // no manual `remove_dir_all` on success or panic paths.
    let root_dir = tempfile::tempdir().expect("tempdir");
    let to_dir = tempfile::tempdir().expect("tempdir");
    let other_dir = tempfile::tempdir().expect("tempdir");
    let root = root_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    let other = other_dir.path().to_path_buf();
    // Bounded setup, insert and flush included: the overall cap (~170 s,
    // under the 180 s nextest kill) bounds the whole multi-phase test, and
    // each await gets up to `WAIT` (>= 120 s) of the remaining time.
    let deadline = Instant::now() + OVERALL;
    let (db, registry) = tokio::time::timeout(remaining(deadline), fixture(&root))
        .await
        .expect("fixture within deadline");
    guard.note("fixture-ready", "engine + registry built");
    std::fs::create_dir_all(root.join("manifest.json.tmp")).expect("blocker dir");
    let blocker_bytes: &[u8] = b"block";
    std::fs::write(root.join("manifest.json.tmp/blocker"), blocker_bytes).expect("blocker file");

    registry.start(to.clone()).expect("start");
    assert!(
        wait_until(|| !registry.status().running, deadline,),
        "the failing run must end on its own"
    );
    guard.note("failed", "run ended with recorded error");
    let snapshot = registry.status();
    assert!(
        snapshot.last_error.is_some(),
        "the manifest-write conflict must be recorded as the last error"
    );
    assert_eq!(
        snapshot.stage,
        BackupStage::Failed,
        "the failed stage must surface typed in the status"
    );
    assert_eq!(
        format!("{:?}", snapshot.stage),
        "Failed",
        "the failed stage must surface in the status"
    );
    assert_eq!(
        snapshot.progress.failed_stage,
        Some(BackupStage::Verifying),
        "the manifest write happens in the Verifying stage"
    );
    assert_eq!(
        std::fs::read(root.join("manifest.json.tmp/blocker")).expect("blocker bytes"),
        blocker_bytes,
        "the failed run must not touch the source-side blocker"
    );
    assert_eq!(
        snapshot.dest_dir, None,
        "a failed run records no snapshot dir"
    );
    assert_eq!(
        std::fs::read_dir(&to).expect("to dir").count(),
        0,
        "the failed run must have cleaned the destination root"
    );
    guard.note("cleaned", "destination root empty after failure");

    // The engine is fully open: with the blocker removed and one more row
    // typed into the SAME live engine (a bounded write through the
    // fixture's db, not a new one), a fresh run passes the whole protocol
    // and reaches `Done` with a manifest that `verify_manifest` accepts.
    std::fs::remove_dir_all(root.join("manifest.json.tmp")).expect("remove blocker");
    let mut batch = Batch::new();
    let mut row_c = new_map();
    row_c.insert("n".to_string(), QueryValue::Int(3));
    batch.op(
        "ins_c",
        write::Insert::with_repo("main", "rows").row(QueryValue::Map(row_c)),
    );
    tokio::time::timeout(remaining(deadline), db.execute("app", &batch.build()))
        .await
        .expect("insert after failure within deadline")
        .expect("insert after failure");
    guard.note("insert-returned", "row c typed");
    tokio::time::timeout(remaining(deadline), db.flush_all_waiting())
        .await
        .expect("flush after failure within deadline")
        .expect("flush after failure");
    guard.note("flush-returned", "row c durable");
    // Nextest isolates process-wide metrics. The first (failed) run must not
    // have published a success, so this admin run is the only possible source.
    let before = backup_metrics_snapshot();
    assert_eq!(before.last_success_timestamp_seconds, 0);
    assert_eq!(before.last_bytes, 0);
    let (second_id, destination) = registry
        .start(other.clone())
        .expect("restart after failure");
    assert!(second_id.starts_with("ob-"), "backup id shape: {second_id}");
    assert_ne!(
        second_id, snapshot.backup_id,
        "each start gets a fresh backup id"
    );
    assert_eq!(destination, other);
    assert!(
        wait_until(|| !registry.status().running, deadline,),
        "the second run must complete on its own"
    );
    guard.note("second-backup-finished", "second run ended");
    let after = registry.status();
    assert!(
        after.last_error.is_none(),
        "the second run must be clean: {:?}",
        after.last_error
    );
    assert_eq!(
        after.stage,
        BackupStage::Done,
        "the second run must reach the final stage (typed)"
    );
    assert_eq!(
        format!("{:?}", after.stage),
        "Done",
        "the second run must reach the final stage"
    );
    let dest_dir = after.dest_dir.expect("second run records the snapshot dir");
    assert!(
        dest_dir.join("manifest.json").exists(),
        "the second run wrote a manifest"
    );
    let verified = verify_manifest(&dest_dir).expect("second snapshot must verify");
    // Wall-clock-free stamp invariant. `before.last_success_timestamp_seconds`
    // is 0 (asserted above: the earlier run FAILED, so only this admin run can
    // stamp) and `last_bytes` below is this run's verified report, so the
    // stamp: (1) is nonzero, (2) moved past the value observed BEFORE the run,
    // i.e. it ADVANCED because of THIS run. A broken `note_success` leaves the
    // cell at the pre-run zero (nothing else in this isolated process writes
    // it), so (2) fails; a stamp copied from another run's value also fails
    // (2) because the observed pre-run value was zero.
    let metrics = backup_metrics_snapshot();
    assert!(
        metrics.last_success_timestamp_seconds > 0,
        "the successful admin run must stamp a nonzero last-success value"
    );
    assert!(
        metrics.last_success_timestamp_seconds > before.last_success_timestamp_seconds,
        "the last-success stamp must ADVANCE past the pre-run value (mutant: \
         `note_success` not stamping): before={} after={}",
        before.last_success_timestamp_seconds,
        metrics.last_success_timestamp_seconds
    );
    let repo_bytes: u64 = after
        .progress
        .per_repo
        .iter()
        .map(|repo| repo.bytes_hashed)
        .sum();
    assert!(
        verified.total_bytes > repo_bytes,
        "fixture includes system-store bytes"
    );
    assert_eq!(metrics.last_bytes, verified.total_bytes);
    guard.note("second-run", "Done + verified");
}

/// The finished-run status survives until the next `start` (§4.7): the
/// successful run's stage, snapshot dir, per-repo progress and the manifest
/// (with the facade-derived system-store version) all remain readable, and
/// a new start replaces the record with a fresh id and destination.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_survives_until_next_start_and_shows_stage_progress() {
    let root = tempfile::tempdir().expect("tempdir").keep();
    let deadline = Instant::now() + OVERALL;
    let (_db, registry) = tokio::time::timeout(remaining(deadline), fixture(&root))
        .await
        .expect("fixture within the bound");
    let to = dest_root();

    registry.start(to.clone()).expect("start");
    assert!(
        wait_until(|| !registry.status().running, deadline,),
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
    cancel_and_wait(&registry, deadline);
    let _ = std::fs::remove_dir_all(snapshot.dest_dir.as_ref().expect("dest dir"));
    let _ = std::fs::remove_dir_all(&other);
    let _ = std::fs::remove_dir_all(&root);
}

/// A repository dropped from the live engine between the adapter's step-A
/// catalogue snapshot and `acquire_pins` must fail with the typed
/// `PinAcquire` port error — before the fix this path re-entered the
/// adapter's own pins mutex (self-deadlock), so the typed error and the
/// follow-up successful run on the SAME engine are the regression
/// evidence. The dropped repo's dir stays behind; a fresh registry run
/// completes `Done` and captures only `app/main`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_repo_after_catalogue_fails_pins_typed_and_next_run_completes() {
    // Failure-only diagnostics: the watchdog dumps the phase timeline
    // after 150 s, below the 180 s nextest per-test kill, so a hang
    // reports where it stuck instead of being silently reaped.
    let guard = FlakeGuard::with_watchdog(
        "dropped_repo_after_catalogue_fails_pins_typed_and_next_run_completes",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    // RAII tempdirs kept alive to the end of the test.
    let root_dir = tempfile::tempdir().expect("tempdir");
    let to_dir = tempfile::tempdir().expect("tempdir");
    let root = root_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    // Bounded setup, insert and flush included.
    let deadline = Instant::now() + OVERALL;
    let (db, registry) = tokio::time::timeout(remaining(deadline), fixture(&root))
        .await
        .expect("fixture within the bound");
    tokio::time::timeout(
        remaining(deadline),
        db.add_repo(
            "app",
            RepoConfig::new("extra", BoxRepoFactory::fjall_raw(root.join("repos/extra")))
                .add_table(TableConfig::new("rows")),
        ),
    )
    .await
    .expect("add second repo within the bound")
    .expect("add second repo");

    // The adapter's step-A snapshot: both repos captured.
    let run_state = Arc::new(PortRunState::new());
    // The port bridges facade calls through `Handle::block_on`, which must
    // never run on a runtime worker: drive it from a bounded OS worker.
    // The port is shared as an `Arc` (its methods take `&self`), so the
    // snapshot thread cannot move it away from the later pin-phase uses.
    let port = Arc::new(EngineBackupPort::new(
        Arc::clone(&db),
        root.clone(),
        tokio::runtime::Handle::current(),
        Arc::clone(&run_state),
    ));
    // Named clones of the shared `Arc` port: each spawned thread gets its
    // own clone so neither closure moves the original, which stays alive
    // here for the later pin-phase checks.
    let port_for_catalogue = Arc::clone(&port);
    let (catalogue_tx, catalogue_rx) = std::sync::mpsc::channel();
    let catalogue_thread = std::thread::spawn(move || {
        let _ = catalogue_tx.send(port_for_catalogue.catalogue());
    });
    let sources = catalogue_rx
        .recv_timeout(remaining(deadline))
        .expect("catalogue within the bound")
        .expect("catalogue must succeed");
    catalogue_thread
        .join()
        .expect("catalogue thread joins cleanly");
    let names: Vec<&str> = sources.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"app/main") && names.contains(&"app/extra"),
        "both repos must be in the snapshot: {names:?}"
    );

    // Drop the repo AFTER the snapshot: the port's captured set still
    // names `app/extra`, but there is no live instance to pin.
    let removed = tokio::time::timeout(remaining(deadline), db.remove_repo("app", "extra"))
        .await
        .expect("remove within the bound")
        .expect("remove must not error");
    assert!(removed, "extra must have been removable");

    // Acquire through the port: typed `PinAcquire`, bounded, no deadlock.
    let port_for_pins = Arc::clone(&port);
    let (pins_tx, pins_rx) = std::sync::mpsc::channel();
    let pins_thread = std::thread::spawn(move || {
        let _ = pins_tx.send(port_for_pins.acquire_pins());
    });
    let pins = pins_rx
        .recv_timeout(remaining(deadline))
        .expect("acquire_pins within the bound");
    pins_thread
        .join()
        .expect("acquire_pins thread joins cleanly");
    assert!(
        matches!(pins, Err(PortError::PinAcquire)),
        "a dropped repo must fail pin acquisition typed: {pins:?}"
    );
    // R4, directly on the SAME adapter: the failing acquisition must have
    // released every pin it took (the follow-up registry run below uses a
    // different port instance and proves nothing about this one's pins).
    assert_eq!(
        port.test_held_pins(),
        0,
        "a failed acquire_pins must leave no pins held on the adapter"
    );

    // The engine is fully open: a real registry run on the SAME engine
    // completes `Done`, capturing only `app/main` (the dropped repo's dir
    // is an inert orphan excluded from capture).
    registry.start(to.clone()).expect("start after pin failure");
    assert!(
        wait_until(|| !registry.status().running, deadline,),
        "the follow-up run must complete on its own"
    );
    let snapshot = registry.status();
    assert!(
        snapshot.last_error.is_none(),
        "the follow-up run must be clean: {:?}",
        snapshot.last_error
    );
    assert_eq!(
        format!("{:?}", snapshot.stage),
        "Done",
        "the follow-up run must reach the final stage"
    );
    let repo_progress = &snapshot.progress.per_repo;
    assert_eq!(repo_progress.len(), 1, "one repo captured");
    assert_eq!(repo_progress[0].repo, "app/main");
    assert!(
        repo_progress[0].bytes_hashed > 0,
        "the captured repo's hashed bytes must be visible"
    );
    assert!(
        snapshot
            .dest_dir
            .as_ref()
            .expect("dest dir")
            .join("manifest.json")
            .is_file(),
        "the follow-up run wrote a manifest"
    );
    guard.note("followup-run", "Done");
    let _ = std::fs::remove_dir_all(snapshot.dest_dir.as_ref().expect("dest dir"));
    let _ = std::fs::remove_dir_all(&to);
    let _ = std::fs::remove_dir_all(&root);
}

/// Defect 4 (R2a-4 follow-up): a panic on the detached runner thread must
/// not leave the registry `running` or the §4.8 metrics stuck. The panic
/// is injected through the registry's per-run opt-in step callback
/// (`arm_runner_step_callback`), moved onto THIS run's runner thread only
/// and fired by the engine's shared test-step dispatcher at the
/// `after_recheck` seal-barrier step — mid-protocol, so the unwind proves
/// the WHOLE cleanup chain — the protocol's `RunCleanup` removes the
/// owned output directory and releases the pins, the seal barrier's own
/// RAII releases the pause and reopens the gate, the permit drops, and
/// then the registry's `RunGuard` publishes failed + idle and closes the
/// metrics. A second `start` succeeds to `Done` on the SAME engine and
/// the live source stays usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_panic_publishes_failed_idle_and_second_start_succeeds() {
    // Failure-only diagnostics: the watchdog dumps the phase timeline
    // after 150 s, below the 180 s nextest per-test kill.
    let guard = FlakeGuard::with_watchdog(
        "runner_panic_publishes_failed_idle_and_second_start_succeeds",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    // RAII tempdirs kept alive to the end of the test.
    let root_dir = tempfile::tempdir().expect("tempdir");
    let to_dir = tempfile::tempdir().expect("tempdir");
    let other_dir = tempfile::tempdir().expect("tempdir");
    let root = root_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    let other = other_dir.path().to_path_buf();
    let deadline = Instant::now() + OVERALL;
    let (db, registry) = tokio::time::timeout(remaining(deadline), fixture(&root))
        .await
        .expect("fixture within deadline");
    guard.note("fixture-ready", "engine + registry built");

    registry.arm_runner_step_callback(Box::new(|step| {
        if step == "after_recheck" {
            panic!("injected runner panic at the seal recheck (registry panic-seam test)");
        }
    }));
    registry.start(to.clone()).expect("start");
    assert!(
        wait_until(|| !registry.status().running, deadline),
        "the panicking run must end on its own (guard published the failure)"
    );
    let snapshot = registry.status();
    assert!(
        !snapshot.running,
        "the guard must have left the registry idle"
    );
    assert_eq!(
        format!("{:?}", snapshot.stage),
        "Failed",
        "the guard must publish a failed stage"
    );
    assert_eq!(
        snapshot.progress.failed_stage,
        Some(BackupStage::Sealing),
        "the panic fired inside the seal barrier: stage hint Sealing"
    );
    assert!(
        snapshot
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("panic"),
        "the guard's error must name the panic: {:?}",
        snapshot.last_error
    );
    assert_eq!(snapshot.dest_dir, None, "no snapshot dir on a panic");
    assert_eq!(
        std::fs::read_dir(&to).expect("to dir").count(),
        0,
        "the unwind must remove the protocol-owned output directory"
    );
    guard.note("panic-published", "failed + idle, output cleaned");

    // The §4.8 metrics, BEFORE the next run can touch them: a manual run has
    // no completion channel, so the guard must still close `active` and count
    // the copy failure at its own stage (mutant: accounting keyed on the
    // optional sender — a manual panic then leaves `active` set and loses the
    // failed-stage increment, which the follow-up run can no longer reveal).
    let metrics = backup_metrics_snapshot();
    assert!(
        !metrics.active,
        "a manual run that panics must clear `shamir_backup_active`: {metrics:?}"
    );
    assert!(
        metrics
            .failed_total_by_stage
            .iter()
            .any(|(stage, count)| stage == "sealing" && *count >= 1),
        "the manual panic must be counted once at its failed stage: {metrics:?}"
    );
    assert_eq!(
        metrics.completed_total, 0,
        "a panicking manual run is never a completed copy: {metrics:?}"
    );

    // The source is usable: a bounded write through the SAME engine (the
    // seal barrier's pause/gate RAII was released on the unwind).
    let mut batch = Batch::new();
    let mut row = new_map();
    row.insert("n".to_string(), QueryValue::Int(7));
    batch.op(
        "ins_after_panic",
        write::Insert::with_repo("main", "rows").row(QueryValue::Map(row)),
    );
    tokio::time::timeout(remaining(deadline), db.execute("app", &batch.build()))
        .await
        .expect("write after panic within deadline")
        .expect("write after panic");
    tokio::time::timeout(remaining(deadline), db.flush_all_waiting())
        .await
        .expect("flush after panic within deadline")
        .expect("flush after panic");
    guard.note("source-usable", "write + flush after panic");

    // The permit and the engine's pins/seals are free: a second start
    // succeeds and completes to `Done` on the SAME engine.
    let (second_id, destination) = registry
        .start(other.clone())
        .expect("second start after panic");
    assert!(second_id.starts_with("ob-"), "backup id shape: {second_id}");
    assert_ne!(second_id, snapshot.backup_id, "fresh backup id");
    assert_eq!(destination, other);
    assert!(
        wait_until(|| !registry.status().running, deadline),
        "the second run must complete on its own"
    );
    let after = registry.status();
    assert!(
        after.last_error.is_none(),
        "the second run must be clean: {:?}",
        after.last_error
    );
    assert_eq!(format!("{:?}", after.stage), "Done");
    let dest_dir = after.dest_dir.expect("second run records the snapshot dir");
    assert!(
        dest_dir.join("manifest.json").is_file(),
        "the second run wrote a manifest"
    );
    verify_manifest(&dest_dir).expect("second snapshot must verify");
    guard.note("second-run", "Done + verified after panic recovery");
}
