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
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_test_diag::FlakeGuard;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};
use tokio::runtime::Builder;
use tokio_util::sync::CancellationToken;

use crate::backup_online::registry::production_seal_cap;
use crate::backup_online::{EngineBackupPort, PortRunState};
use crate::sync::durability::backup::online::copy_engine::CopyOptions;
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    run_online_backup, BackupReport, OnlineBackupError, OnlineBackupFailure, SealRecopyCap,
};
use crate::sync::durability::backup::online::wal_capture::{
    CapturedPoint, OnlineBackupPort, PortError, RepoSource,
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
    let db = runtime.block_on(Box::pin(durable_engine_with_wal(&workspace)));

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

// ── Seal shutdown-cancellation regression (R2a follow-up 3) ─────────────
//
// Coordination note: the seal pause goes through the shared backup step
// hook (`crate::backup_online::registry::arm_backup_step`), NOT a locally
// installed `RepoInstance::set_online_backup_step_hook` — that hook is
// first-installer-wins per process and is owned by the shared dispatcher,
// so tests must never install it themselves. Every wait is
// `recv_timeout`-bounded on a `sync_channel(1)` rendezvous; the whole test
// is bounded by one FlakeGuard watchdog. No sleeps.

/// Total wall-clock budget: the FlakeGuard watchdog and every
/// `recv_timeout` derive from it (well under the runner's per-test kill).
const TEST_BOUND: Duration = Duration::from_secs(120);

/// R2a follow-up 3, live regression: REAL engine + REAL
/// `EngineBackupPort` + the real `run_online_backup` protocol, with a
/// writer transaction STAGED on the repo's table (`begin_tx(Snapshot)` +
/// `insert_tx`) BEFORE the backup thread starts, its `commit_tx` future
/// boxed once and driven through the whole sequence.
///
/// Proven: while the seal barrier is paused at the seal-only `after_drain`
/// step the SAME boxed commit future polls `Pending` (the admission gate
/// is closed); cancelling the ROOT shutdown token while the bridge stays
/// parked does NOT reopen it (still `Pending` — cancellation is observed
/// only at step boundaries); after release the SAME future completes and
/// the committed row reads back; the run fails with EXACTLY
/// `OnlineBackupError::Cancelled { stage: BackupStage::Sealing }` and its
/// owned destination is removed (fail-closed, no snapshot).
#[test]
fn seal_shutdown_cancellation_pauses_bridge_without_premature_reopen() {
    let _guard = FlakeGuard::with_watchdog(
        "seal_shutdown_cancellation_pauses_bridge_without_premature_reopen",
        TEST_BOUND,
    );

    // ONE multi-thread runtime shared by fixture, port bridge and writer.
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");

    // Live TempDirs: kept by value; Drop cleans up at scope end.
    let workspace = tempfile::tempdir().expect("tempdir");
    let destination = tempfile::tempdir().expect("tempdir");
    let db = runtime.block_on(Box::pin(durable_engine_with_wal(workspace.path())));

    let run_state = Arc::new(PortRunState::new());
    let port = Arc::new(EngineBackupPort::new(
        Arc::clone(&db),
        workspace.path().to_path_buf(),
        runtime.handle().clone(),
        Arc::clone(&run_state),
    ));

    // ── Pre-staged writer (before the backup thread exists) ─────────
    let repo = db
        .get_db("app")
        .expect("db app")
        .get_repo("main")
        .expect("repo main");
    let (tx, snapshot) = runtime
        .block_on(repo.begin_tx(IsolationLevel::Snapshot))
        .expect("begin_tx");
    let table = runtime
        .block_on(repo.get_table("rows"))
        .expect("table rows");
    let mut tx = tx;
    let rid = runtime
        .block_on(table.insert_tx(&InnerValue::Int(3), Some(&mut tx)))
        .expect("stage insert_tx");

    // The SAME boxed commit future through every phase below (the
    // Result is kept: the success assert happens under the overall
    // bound). The snapshot guard moves in with it — it must outlive
    // the commit.
    let writer_repo = repo.clone();
    let mut writer = Box::pin(async move {
        let _snapshot_guard = snapshot;
        writer_repo.commit_tx(tx).await
    });

    // ── Production cancellation wiring: root shutdown token ─────────
    let shutdown = CancellationToken::new();
    let cancel = CancelFlag::from_shutdown(&shutdown);

    // Pause handoff: the hook closure (running on the backup thread,
    // inside the barrier) announces the pause and blocks on release;
    // both channels are `sync_channel(1)` bounded rendezvous.
    let (paused_tx, paused_rx) = mpsc::sync_channel::<&'static str>(1);
    let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);

    let port_for_run = Arc::clone(&port);
    let (done_tx, done_rx) = mpsc::channel();
    let source_root = workspace.path().to_path_buf();
    let dest_root = destination.path().to_path_buf();
    let backup_thread = std::thread::spawn(move || {
        // Shared dispatcher arm — BEFORE the run, on the thread that
        // drives the protocol (and fires the barrier hook).
        crate::backup_online::registry::arm_backup_step("after_drain", move || {
            let _ = paused_tx.send("after_drain");
            // Parked HERE with the admission gate closed until release
            // (bounded: never park forever on a lost release).
            let _ = release_rx.recv_timeout(TEST_BOUND);
        });
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
        let _ = done_tx.send(result);
    });

    // The barrier reached `after_drain`: gate closed, bridge parked.
    let fired = paused_rx
        .recv_timeout(TEST_BOUND)
        .expect("the seal barrier must reach the armed step within the bound");
    assert_eq!(fired, "after_drain");

    // BEFORE cancellation: the SAME commit future is Pending — the gate
    // is closed by the barrier (a noop-waker poll, in runtime context,
    // never completing the future).
    let pending_before = runtime
        .handle()
        .block_on(async { futures_util::poll!(writer.as_mut()).is_pending() });
    assert!(
        pending_before,
        "the staged commit must be Pending while the gate is closed"
    );

    // Cancel the ROOT shutdown token while the bridge is still parked.
    shutdown.cancel();

    // Bridge still parked: the SAME future is polled again and must be
    // STILL Pending — cancellation cannot reopen the gate prematurely;
    // it is observed only at the engine's next step boundary.
    let pending_after = runtime
        .handle()
        .block_on(async { futures_util::poll!(writer.as_mut()).is_pending() });
    assert!(
        pending_after,
        "the commit must remain Pending after cancellation while parked"
    );

    // Release the pause: the engine's next checkpoint observes the
    // cancelled closure, fails the seal CLOSED and reopens the gate on
    // the ordinary error path (never by dropping the barrier future).
    release_tx
        .send(())
        .expect("the bridge must still be parked on the release channel");

    // Await the SAME writer future to completion, under the overall
    // bound (the remaining budget, not a fresh cap). ONE time origin for
    // the whole test: tokio's `Instant` (what `timeout_at` takes).
    let deadline = tokio::time::Instant::now() + TEST_BOUND;
    let outcome = runtime
        .block_on(Box::pin(async {
            tokio::time::timeout_at(deadline, writer.as_mut()).await
        }))
        .expect("the staged commit must complete within the overall bound")
        .expect("the staged commit succeeds after the gate reopens");
    // InnerValue::Int and TxOutcome::materialized verified against the
    // engine sources (value.rs variant; tx_outcome.rs method).
    assert!(outcome.materialized(), "the staged row must materialize");

    // Readback: the committed row is visible on a fresh repo handle.
    let reader = db
        .get_db("app")
        .expect("db app")
        .get_repo("main")
        .expect("repo main");
    let table = runtime
        .block_on(reader.get_table("rows"))
        .expect("table rows");
    let stored = runtime
        .block_on(table.get(rid))
        .expect("committed row must read back");
    assert_eq!(stored, InnerValue::Int(3), "the row must read back as n=3");

    // The run failed with the typed seal cancellation at `Sealing` (the
    // cleanup slot is not asserted here) and its owned destination is
    // gone — fail-closed, no snapshot.
    let result = done_rx
        .recv_timeout(TEST_BOUND)
        .expect("the protocol run must finish within the bound");
    let error = result.expect_err("a cancelled seal must fail the run");
    assert!(
        matches!(
            &error,
            OnlineBackupError::Cancelled {
                stage: BackupStage::Sealing,
                ..
            }
        ),
        "exactly the Sealing-stage cancellation is required, got: {error}"
    );
    backup_thread.join().expect("backup thread must not panic");
    // The catalogue captured the one file-backed repo (a cancelled seal
    // on an EMPTY catalogue would be a dead test).
    assert_eq!(
        run_state.snapshot_count(),
        1,
        "the file-backed repo must be in the step-A catalogue"
    );
    assert!(
        fs::read_dir(destination.path())
            .expect("destination root")
            .next()
            .is_none(),
        "a cancelled run must leave no snapshot under the destination"
    );

    drop(port);
    drop(db);
    drop(runtime);
}

// ── Seal re-copy cap semantics (R2a follow-up 4) ────────────────────────
//
// An ISOLATED synthetic file-backed port (no engine, no Fjall, no
// threads): the protocol is driven directly on the test thread over a
// planted source tree whose WAL segment is >64 MiB from the FIRST pass
// (canonical numeric stem `00000000.wal` + `.meta` sidecar — the manifest
// only admits numeric stems). The seal barrier flips ONE byte of that
// segment, so the destination digests authenticated before the hold no
// longer match and the seal re-copy must rewrite the WHOLE file — the
// full-file-rewrite shape the byte cap was wrongly judged against. Each
// run XORs byte 0 with 0xFF (`0xA5 → 0x5A → 0xA5 → …`), so EVERY run
// sees a fresh change; the per-run byte is asserted exactly. The
// production cap (`production_seal_cap`: `max_bytes = None`, 8 s) must
// complete; the FORMER 64 MiB cap and an explicitly tiny cap (clock
// disabled, `max_seconds = None`, so the typed message is provably the
// BYTE cap, not the deadline) must fail typed at `Sealing`.

const MIIB: u64 = 1024 * 1024;
/// The WAL segment under test: >64 MiB from the very first pass.
const BIG_WAL_BYTES: u64 = 65 * MIIB;
/// The FORMER production byte cap (the negative control).
const FORMER_BYTE_CAP: u64 = 64 * MIIB;

/// Synthetic file-backed port: one repo, counters instead of engine pins.
struct BigWalPort {
    store_dir: PathBuf,
    wal_dir: PathBuf,
    big_wal: PathBuf,
    pins: std::cell::Cell<u32>,
}

fn consistency_point() -> CapturedPoint {
    CapturedPoint {
        wal_position: 42,
        store_dir: "repos/main".to_string(),
        schema_version: 1,
    }
}

impl OnlineBackupPort for BigWalPort {
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError> {
        Ok(vec![RepoSource {
            name: "app/main".to_string(),
            store_dir: self.store_dir.clone(),
            wal_dir: Some(self.wal_dir.clone()),
        }])
    }

    fn acquire_pins(&self) -> Result<(), PortError> {
        self.pins.set(self.pins.get() + 1);
        Ok(())
    }

    fn capture_point(&self, _repo: &str) -> Result<CapturedPoint, PortError> {
        Ok(consistency_point())
    }

    fn capture_system_store(&self) -> Result<u64, PortError> {
        Ok(1)
    }

    fn seal_point(&self, _repo: &str) -> Result<CapturedPoint, PortError> {
        Ok(consistency_point())
    }

    /// The seal barrier: TOGGLES one byte of the big WAL segment (reads
    /// it back and XORs with 0xFF), so every run flips `0xA5 ↔ 0x5A` and
    /// always presents a fresh change — the destination digests
    /// authenticated before the hold no longer match and the seal re-copy
    /// must rewrite the WHOLE file — the full-file-rewrite shape the byte
    /// cap was wrongly judged against. (Writing a constant byte would
    /// make the second and third runs see an unchanged source and skip
    /// the re-copy entirely.)
    fn seal_point_before(
        &self,
        _repo: &str,
        _deadline: Option<Instant>,
        _cancel: &CancelFlag,
    ) -> Result<CapturedPoint, PortError> {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.big_wal)
            .map_err(|source| PortError::Seal {
                repo: "app/main".to_string(),
                source,
            })?;
        let mut byte = [0_u8; 1];
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_exact(&mut byte))
            .map_err(|source| PortError::Seal {
                repo: "app/main".to_string(),
                source,
            })?;
        byte[0] ^= 0xFF;
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.write_all(&byte))
            .map_err(|source| PortError::Seal {
                repo: "app/main".to_string(),
                source,
            })?;
        Ok(consistency_point())
    }

    fn end_seal(&self, _repo: &str) {}

    fn release_pins(&self) {
        self.pins.set(self.pins.get().saturating_sub(1));
    }
}

/// The cap scenarios over the synthetic port: former 64 MiB cap and tiny
/// cap fail TYPED (byte message, no clock), the production cap completes
/// PAST the former limit and verifies.
#[test]
fn seal_recopy_byte_cap_semantics_on_synthetic_file_port() {
    let source = tempfile::tempdir().expect("tempdir");
    let store_dir = source.path().join("repos/main");
    let wal_dir = source.path().join("repos/main.shamirwal");
    fs::create_dir_all(&store_dir).expect("store dir");
    fs::create_dir_all(&wal_dir).expect("wal dir");
    fs::write(store_dir.join("store.bin"), b"store-bytes").expect("store file");
    fs::write(wal_dir.join("00000000.meta"), b"sidecar").expect("wal sidecar");
    let big_wal = wal_dir.join("00000000.wal");
    fs::write(&big_wal, vec![0xA5_u8; BIG_WAL_BYTES as usize]).expect("big wal");
    let port = BigWalPort {
        store_dir: store_dir.clone(),
        wal_dir: wal_dir.clone(),
        big_wal: big_wal.clone(),
        pins: std::cell::Cell::new(0),
    };
    let cancel = CancelFlag::default();

    // Negative control: the FORMER production byte cap (no clock, so the
    // typed message is provably the byte cap) fails the full rewrite.
    let former = SealRecopyCap {
        max_bytes: Some(FORMER_BYTE_CAP),
        max_seconds: None,
    };
    let destination_former = tempfile::tempdir().expect("tempdir");
    let error = run_online_backup(
        &port,
        source.path(),
        destination_former.path(),
        &cancel,
        &mut BackupProgress::new(),
        &CopyOptions::default(),
        &former,
    )
    .expect_err("the former 64 MiB cap must fail a >64 MiB full-file rewrite");
    let OnlineBackupError::Failed {
        stage,
        source: OnlineBackupFailure::RecopyCap(message),
        ..
    } = &error
    else {
        panic!("the byte cap must fail typed at Sealing, got: {error}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert!(
        message.contains("bytes, cap"),
        "the failure must name the BYTE cap, not the clock: {message}"
    );
    assert_eq!(port.pins.get(), 0, "pins must be released on the failure");
    assert!(
        fs::read_dir(destination_former.path())
            .expect("destination root")
            .next()
            .is_none(),
        "a cap-tripped run must leave no snapshot"
    );

    // An EXPLICITLY tiny byte cap gives the same typed failure.
    let tiny = SealRecopyCap {
        max_bytes: Some(1024),
        max_seconds: None,
    };
    let destination_tiny = tempfile::tempdir().expect("tempdir");
    let error = run_online_backup(
        &port,
        source.path(),
        destination_tiny.path(),
        &cancel,
        &mut BackupProgress::new(),
        &CopyOptions::default(),
        &tiny,
    )
    .expect_err("a tiny byte cap must fail the rewrite");
    let OnlineBackupError::Failed {
        stage,
        source: OnlineBackupFailure::RecopyCap(message),
        ..
    } = &error
    else {
        panic!("the tiny cap must fail typed at Sealing, got: {error}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert!(
        message.contains("bytes, cap"),
        "the tiny failure must name the BYTE cap, not the clock: {message}"
    );
    assert_eq!(port.pins.get(), 0, "pins must be released on the failure");
    assert!(
        fs::read_dir(destination_tiny.path())
            .expect("destination root")
            .next()
            .is_none(),
        "a cap-tripped run must leave no snapshot"
    );

    // The PRODUCTION cap (`max_bytes = None`, 8 s per repo) completes the
    // same full-file rewrite PAST the former 64 MiB limit: the snapshot
    // carries the WHOLE seal-time file and verifies. Three seal barriers
    // have fired, so byte 0 is `0xA5 ^ 0xFF ^ 0xFF ^ 0xFF = 0x5A`; the
    // rest is still the planted first-pass fill.
    let destination_ok = tempfile::tempdir().expect("tempdir");
    let report = run_online_backup(
        &port,
        source.path(),
        destination_ok.path(),
        &cancel,
        &mut BackupProgress::new(),
        &CopyOptions::default(),
        &production_seal_cap(),
    )
    .expect("the production cap must complete past the former 64 MiB limit");
    let snapshot_big = report.dest_dir.join("repos/main.shamirwal/00000000.wal");
    let copied = fs::read(&snapshot_big).expect("the rewritten big WAL in the snapshot");
    assert_eq!(
        copied.len() as u64,
        BIG_WAL_BYTES,
        "the snapshot must carry the WHOLE rewritten file"
    );
    assert_eq!(
        copied[0], 0x5A,
        "the third seal-time byte must be in the snapshot"
    );
    assert!(
        copied[1..].iter().all(|&b| b == 0xA5),
        "the rest of the rewritten file must match the first pass"
    );
    verify_manifest(&report.dest_dir).expect("the snapshot must verify");
    assert_eq!(port.pins.get(), 0, "pins must be released on success");
}
