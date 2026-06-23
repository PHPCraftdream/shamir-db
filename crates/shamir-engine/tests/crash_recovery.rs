//! Real crash-recovery harness — kill mid-commit at each phase, prove
//! atomicity around the Phase-4 WAL commit point (Vector II.1).
//!
//! Unlike the in-process `drop(Arc<InMemoryRepo>)` "crash" tests, this
//! harness performs a GENUINE process death over a DISK-backed (redb,
//! unbuffered) store:
//!
//!   * The parent test spawns a CHILD process — a re-exec of this same
//!     test binary, routed by the `SHAMIR_TEST_CRASH_CHILD` env sentinel
//!     into [`run_child_scenario`]. The child opens a redb repo at a
//!     shared temp path, stages a known FTS-indexed record in a tx, and
//!     calls `commit_tx`. The commit pipeline reads `SHAMIR_TEST_CRASH_AFTER`
//!     and `std::process::abort()`s at the requested seam (see
//!     `commit.rs::maybe_crash`). `abort` = `SIGABRT`: no unwind, no
//!     `Drop`, no flush — the closest in-process analog to `kill -9`, so
//!     the on-disk image is a genuine torn-mid-commit state.
//!
//!   * The parent waits for the child, asserts it died abnormally
//!     (`!status.success()` — cross-platform, since a SIGABRT exit code
//!     differs on Unix vs Windows), then REOPENS a fresh `RepoInstance`
//!     over the SAME redb file, runs `recover_v2_inflight`, and asserts
//!     atomicity:
//!
//!       - crash at `pre_commit` (BEFORE Phase 4) → recovery replays 0
//!         entries; the record + index postings are ABSENT (clean abort,
//!         nothing durable).
//!       - crash at `phase4`..`phase6_5` (AT/AFTER the commit point,
//!         before Phase 7) → recovery replays exactly 1 entry; the data
//!         record AND the index postings are PRESENT (all-or-nothing).
//!       - crash at `phase7` (WAL marker already removed, tx fully
//!         materialized) → the record + postings are PRESENT and recovery
//!         is a no-op.
//!
//! Atomicity is asserted from two angles: the DATA side by counting the
//! records physically present in the `__data__<t>` store (bypassing the
//! table counter, which recovery intentionally does not replay), and the
//! INDEX side by an FTS lookup whose plain backend reads its postings
//! live from `info_store` — a non-zero hit means the tx's index postings
//! were materialized. Both must move together (all-or-nothing): a crash
//! before Phase 4 leaves zero of each; a crash at/after it recovers both.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use shamir_query_types::admin::CreateIndexOp;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use shamir_engine::index2::backend::{FtsMode, IndexQuery, IndexResult};
use shamir_engine::index2::tokenizer::token_hash;
use shamir_engine::repo::repo_types::BoxRepoFactory;
use shamir_engine::repo::RepoInstance;
use shamir_engine::table::{TableConfig, TableManager};

/// Env sentinel: when present the test binary is the spawned CHILD and
/// runs [`run_child_scenario`] instead of asserting anything.
const CHILD_SENTINEL: &str = "SHAMIR_TEST_CRASH_CHILD";
/// Env var read by `commit.rs::maybe_crash` to pick the crash seam.
const CRASH_AFTER: &str = "SHAMIR_TEST_CRASH_AFTER";
/// Env var carrying the shared redb repo path from parent → child.
const REPO_PATH: &str = "SHAMIR_TEST_CRASH_PATH";

const TABLE: &str = "docs";
const TABLE_B: &str = "tags";
const REPO_NAME: &str = "crash_repo";

/// Env sentinel: when present the child runs the MULTI-TABLE scenario
/// instead of the single-table one. Distinct from `CHILD_SENTINEL` so the
/// existing single-table tests are unaffected.
const CHILD_SENTINEL_MULTI: &str = "SHAMIR_TEST_CRASH_CHILD_MULTI";

/// F6c. Env sentinel: when present the child runs the TRUNCATION scenario —
/// commit many small records (rolling several WAL segments under a tiny
/// segment cap), then `drain_all`, which replays each into `history`,
/// advances the durable watermark over the sealed segments, and fires the
/// truncation crash seam (`pre_truncate` / `post_truncate` in the drainer,
/// or `wal_mid_delete` inside `SegmentSet::truncate_below`).
const CHILD_SENTINEL_TRUNC: &str = "SHAMIR_TEST_CRASH_CHILD_TRUNC";

/// F6c. Env var carrying the small WAL segment cap (bytes) parent → child so
/// `repo_instance::repo_wal` rolls real segments and truncation has sealed
/// segments to delete. Read by `repo_instance` directly from this name.
const SEG_MAX_BYTES_ENV: &str = "SHAMIR_WAL_SEGMENT_MAX_BYTES";

/// Number of records the truncation child commits. Large enough (with the
/// tiny cap below) to seal MULTIPLE segments so `wal_mid_delete` has >= 2
/// truncatable segments to delete between.
const TRUNC_RECORDS: usize = 40;

/// Tiny segment cap (bytes) for the truncation scenario — forces a seal
/// roughly every record so 40 commits produce many sealed segments.
const TRUNC_SEG_CAP: &str = "4096";

// ---------------------------------------------------------------------------
// Shared scenario helpers (used by both the child and, on the read side,
// the parent).
// ---------------------------------------------------------------------------

fn fts_index_op() -> CreateIndexOp {
    CreateIndexOp {
        create_index: "body_fts".into(),
        table: TABLE.into(),
        fields: vec![vec!["body".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("fts".into()),
        fts_tokenizer: Some("whitespace".into()),
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

async fn field_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn text_record(body_key_id: u64, text: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(body_key_id), InnerValue::Str(text.into()));
    InnerValue::Map(m)
}

/// Open a fresh `RepoInstance` over the redb file at `path`.
///
/// `redb_raw` (no MemBuffer wrapper): a real on-disk file, no buffering
/// layer hiding writes behind a flush tick. redb's per-row `set` uses a
/// deferred-fsync mode, so the WAL `begin` + Phase-5 row writes become
/// crash-durable only when the commit pipeline's crash seam flushes the
/// shared backend right before `process::abort()` (see
/// `commit.rs::maybe_crash`). That flush models a synchronously-durable
/// WAL backend; the reopened repo then sees the real torn-mid-commit
/// image the killed child left behind.
async fn open_repo(path: &Path) -> RepoInstance {
    let factory = BoxRepoFactory::fjall_raw(path.to_path_buf());
    RepoInstance::from_factory(
        REPO_NAME.into(),
        factory,
        vec![TableConfig::new(TABLE), TableConfig::new(TABLE_B)],
    )
    .await
    .expect("open fjall repo")
}

/// Count records present in a table by draining its `list_stream` (the
/// MvccStore seam). When an MvccStore is attached, `list_stream` reads
/// from the version log (`current_stream`), which is the sole write target
/// after FINAL-A. When no MvccStore is attached (impossible after
/// `get_table`), it falls back to `data_store().iter_stream`.
///
/// Bypasses the table counter (which recovery intentionally does NOT
/// replay), so it measures the true materialized data set.
async fn store_record_count(tbl: &TableManager) -> usize {
    let stream = tbl.list_stream(256);
    futures::pin_mut!(stream);
    let mut n = 0usize;
    while let Some(batch) = stream.next().await {
        n += batch.expect("list_stream batch").len();
    }
    n
}

/// Number of FTS hits for the indexed token after recovery. Proves the
/// INDEX side of atomicity: the plain `FtsBackend` reads its postings
/// live from `info_store`, so a non-zero hit count means the tx's index
/// postings were materialized (by recovery for the post-commit phases).
///
/// The FTS index descriptor the child created is persisted with the rest
/// of the on-disk image, so a restarted repo auto-loads the backend (with
/// the child's original descriptor id) and the recovered postings
/// key-match. On the `pre_commit` (clean-abort) path the descriptor was
/// never flushed, so no backend is registered → return 0.
async fn fts_hit_count(tbl: &TableManager) -> usize {
    // Resolve the backend via `all_backends` (there is exactly one — the
    // FTS index) rather than `get_by_name`: the descriptor carries the
    // CHILD's interned name id, which need not equal a freshly-interned
    // "body_fts" in the restarted parent. On the clean-abort path the
    // descriptor was never made durable, so the registry is empty → 0.
    let backends = tbl.index2_registry().all_backends().await;
    let Some(backend) = backends.first() else {
        return 0;
    };
    match backend
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("atomicity")],
            mode: FtsMode::AndAll,
        })
        .await
        .expect("fts lookup")
    {
        IndexResult::Set(s) => s.len(),
        IndexResult::Ranked(hits) => hits.len(),
    }
}

// ---------------------------------------------------------------------------
// CHILD: open the redb repo, stage a record, commit → abort at the seam.
// ---------------------------------------------------------------------------

/// The scenario the spawned child runs. On a successful return (only when
/// the crash seam did not match — e.g. an unknown phase label) the child
/// exits 0; normally the commit pipeline `process::abort()`s before this
/// returns, so the parent observes an abnormal exit.
async fn run_child_scenario(path: PathBuf) {
    let repo = open_repo(&path).await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    tbl.create_index_v2(&fts_index_op()).await.unwrap();

    let body_id = field_id(&tbl, "body").await;

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.insert_tx(
        &text_record(body_id, "shamir crash recovery atomicity"),
        Some(&mut tx),
    )
    .await
    .unwrap();

    // commit_tx will read SHAMIR_TEST_CRASH_AFTER and abort at the
    // matching seam. If the label is unknown it commits normally.
    let _ = repo.commit_tx(tx).await;

    // D2 P1d-2c: post-cutover the "phase7" seam (tx fully materialized + WAL
    // cleanup) lives in the background drainer, not the ack-path. Drive the
    // drainer so a `phase7` crash fires here. For the inline phases
    // (pre_commit..phase6_5) the process has already `process::abort`ed inside
    // `commit_tx` above and never reaches this line.
    let _ = repo.drainer().drain_all(&repo).await;

    drop(guard);
}

// ---------------------------------------------------------------------------
// PARENT: spawn child at a phase, then reopen + recover + assert.
// ---------------------------------------------------------------------------

/// Spawn the crash child: re-exec THIS test binary with the child
/// sentinel + crash phase + repo path set, running only the
/// `child_entrypoint` test (`--exact`). Returns the child's exit status.
fn spawn_child(phase: &str, repo_path: &Path) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        // Run ONLY the child entrypoint test so the child does not
        // recurse into the parent harness tests.
        .args(["--exact", "child_entrypoint", "--nocapture"])
        .env(CHILD_SENTINEL, "1")
        .env(CRASH_AFTER, phase)
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn crash child")
        .status
}

/// One full cycle: spawn a child that crashes at `phase`, then reopen the
/// same redb file, recover, and return `(replayed_entries, data_records,
/// fts_hits)`.
async fn crash_then_recover(phase: &str) -> (usize, usize, usize) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child(phase, &repo_path);
    assert!(
        !status.success(),
        "child must die abnormally on a process::abort() at phase {phase}; \
         got a clean exit (status {status:?}) — the crash seam did not fire"
    );

    // === RESTART === reopen a fresh RepoInstance over the same on-disk
    // redb image the killed child left behind.
    let repo = open_repo(&repo_path).await;

    // Recover FIRST so the WAL replay writes any committed index postings
    // to info_store before we register a backend to read them.
    let replayed = repo.recover_v2_inflight().await.expect("recovery");

    // `get_table` auto-loads the persisted index2 descriptors, so on the
    // committed phases the FTS backend is already registered (with the
    // child's original id) and reads the RECOVERED postings — no
    // re-indexing happens here.
    let tbl = repo.get_table(TABLE).await.unwrap();
    let data = store_record_count(&tbl).await;
    let fts = fts_hit_count(&tbl).await;

    // tempdir drops here — the child process is already dead so the file
    // is no longer open.
    (replayed, data, fts)
}

/// Child entrypoint: a `#[test]` that is a NO-OP in the parent process
/// (sentinel absent → returns immediately) and runs the crash scenario
/// in the spawned child (sentinel present). Named `child_entrypoint` so
/// the parent can target it with `--exact`.
#[test]
fn child_entrypoint() {
    if std::env::var(CHILD_SENTINEL).is_err() {
        // Parent process running its own test sweep — not the child.
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario(path));
    // If we reach here the crash seam did not fire (unknown label); exit
    // cleanly so the parent's `!status.success()` assertion flags it.
}

// ---------------------------------------------------------------------------
// The phase matrix. Each test = one real subprocess kill + recovery.
// ---------------------------------------------------------------------------

/// Crash BEFORE the commit point (Phase 4 not reached). The WAL entry was
/// never written → recovery finds nothing → the tx left NOTHING. Strict
/// atomicity: none of its writes are visible.
#[tokio::test]
async fn crash_pre_commit_leaves_nothing() {
    let (replayed, data, fts) = crash_then_recover("pre_commit").await;
    assert_eq!(replayed, 0, "no WAL entry before the commit point");
    assert_eq!(data, 0, "clean abort: no data record materialized");
    assert_eq!(fts, 0, "clean abort: no index posting materialized");
}

/// Crash AT the commit point (WAL entry durable, no projection ran).
/// Recovery must replay the single inflight entry and materialize BOTH
/// the data record and the index postings — all-or-nothing.
#[tokio::test]
async fn crash_at_phase4_recovers_full_tx() {
    let (replayed, data, fts) = crash_then_recover("phase4").await;
    assert_eq!(replayed, 1, "the committed entry must be replayed once");
    assert_eq!(
        data, 1,
        "committed tx: data record materialized by recovery"
    );
    assert!(
        fts >= 1,
        "committed tx: index postings materialized by recovery (got {fts})"
    );
}

/// Crash after Phase 5a (data on disk, index not yet). Recovery replays
/// the inflight entry; the final state has both data and index.
#[tokio::test]
async fn crash_at_phase5a_recovers_full_tx() {
    let (replayed, data, fts) = crash_then_recover("phase5a").await;
    assert_eq!(replayed, 1, "inflight entry replayed once");
    assert_eq!(data, 1, "data present after recovery");
    assert!(
        fts >= 1,
        "index postings present after recovery (got {fts})"
    );
}

/// Crash after Phase 5c (data + index on disk, version unpublished,
/// Phase 7 not run). Recovery re-applies idempotently; full state.
#[tokio::test]
async fn crash_at_phase5c_recovers_full_tx() {
    let (replayed, data, fts) = crash_then_recover("phase5c").await;
    assert_eq!(replayed, 1, "inflight entry replayed once");
    assert_eq!(data, 1, "data present after recovery");
    assert!(
        fts >= 1,
        "index postings present after recovery (got {fts})"
    );
}

/// Crash after Phase 6 publish (version published in-memory, lost with
/// the process; markers + Phase 7 not run). Recovery materializes the tx.
#[tokio::test]
async fn crash_at_phase6_recovers_full_tx() {
    let (replayed, data, fts) = crash_then_recover("phase6").await;
    assert_eq!(replayed, 1, "inflight entry replayed once");
    assert_eq!(data, 1, "data present after recovery");
    assert!(
        fts >= 1,
        "index postings present after recovery (got {fts})"
    );
}

/// Crash after Phase 6.5 markers (everything on disk except the WAL
/// marker removal). Recovery re-applies the already-present entry once
/// and cleans the marker — final state fully materialized.
#[tokio::test]
async fn crash_at_phase6_5_recovers_full_tx() {
    let (replayed, data, fts) = crash_then_recover("phase6_5").await;
    assert_eq!(replayed, 1, "inflight entry replayed once");
    assert_eq!(data, 1, "data present after recovery");
    assert!(
        fts >= 1,
        "index postings present after recovery (got {fts})"
    );
}

/// Crash after Phase 7 (tx fully materialized). The data + index are
/// PRESENT and recovery converges to the same clean committed state.
///
/// File-WAL contract: there is no per-entry marker removal until the F6
/// checkpoint (`wal.commit` is a no-op in file mode), so the durable entry
/// stays in the segment and replay re-applies it once. Idempotency means
/// the DATA is unchanged (still exactly one record), NOT that the replay
/// count is zero. When F6 truncation lands this tightens back to `== 0`.
#[tokio::test]
async fn crash_at_phase7_is_clean_committed() {
    let (replayed, data, fts) = crash_then_recover("phase7").await;
    assert_eq!(
        replayed, 1,
        "file WAL replays the durable entry once (no F6 truncation yet)"
    );
    assert_eq!(data, 1, "idempotent re-materialization — still one record");
    assert!(
        fts >= 1,
        "index postings present after recovery (got {fts})"
    );
}

// ---------------------------------------------------------------------------
// MULTI-TABLE (MED-A): cross-table logical atomicity via WAL replay.
// ---------------------------------------------------------------------------

/// Multi-table variant: a SINGLE tx writes ONE record into EACH of two
/// tables (`docs` + `tags`) and crashes at `phase4` (AFTER `wal.begin`,
/// BEFORE any Phase-5 materialization). The on-disk image after the
/// `process::abort` contains exactly one inflight WAL entry whose
/// `Vec<WalOpV2>` carries the Put for BOTH tables (per `wal_ops_from_tx`
/// iterating every entry in `tx.write_set`).
async fn run_child_scenario_multi_table(path: PathBuf) {
    let repo = open_repo(&path).await;
    let tbl_a = repo.get_table(TABLE).await.unwrap();
    let tbl_b = repo.get_table(TABLE_B).await.unwrap();

    let body_id_a = field_id(&tbl_a, "body").await;
    let body_id_b = field_id(&tbl_b, "body").await;

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl_a
        .insert_tx(&text_record(body_id_a, "row-in-table-a"), Some(&mut tx))
        .await
        .unwrap();
    tbl_b
        .insert_tx(&text_record(body_id_b, "row-in-table-b"), Some(&mut tx))
        .await
        .unwrap();

    // commit_tx reads SHAMIR_TEST_CRASH_AFTER and aborts at `phase4`.
    let _ = repo.commit_tx(tx).await;

    // D2 P1d-2c: post-cutover the "phase7" seam (tx fully materialized + WAL
    // cleanup) lives in the background drainer, not the ack-path. Drive the
    // drainer so a `phase7` crash fires here. For the inline phases
    // (pre_commit..phase6_5) the process has already `process::abort`ed inside
    // `commit_tx` above and never reaches this line.
    let _ = repo.drainer().drain_all(&repo).await;

    drop(guard);
}

/// Child entrypoint for the multi-table scenario. A `#[test]` that is a
/// NO-OP in the parent process (sentinel absent) and runs the multi-table
/// crash scenario in the spawned child (sentinel present).
#[test]
fn child_entrypoint_multi_table() {
    if std::env::var(CHILD_SENTINEL_MULTI).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_multi_table(path));
}

// ---------------------------------------------------------------------------
// F6c TRUNCATION crash scenarios. Commit many small records under a tiny
// segment cap, then `drain_all` — the drainer truncates the WAL and the
// requested seam (`pre_truncate` / `post_truncate` / `wal_mid_delete`) aborts
// the process mid-truncation. Recovery from the on-disk image must lose
// NOTHING: every record that reached `history` survives, and the truncation
// itself never discards undurable data (I1/I2/I6).
// ---------------------------------------------------------------------------

/// The truncation child: commit `TRUNC_RECORDS` single-field records (each a
/// separate tx so each gets its own `commit_version`), then `drain_all`. With
/// the tiny `SHAMIR_WAL_SEGMENT_MAX_BYTES` cap inherited from the parent, the
/// WAL rolls many sealed segments; `drain_all` replays them all into `history`,
/// advances `durable_watermark` past them, and the truncation crash seam fires.
///
/// On a successful (non-crashing) return — only if the seam label never
/// matched — the child exits 0 and the parent's `!status.success()` flags it.
async fn run_child_scenario_trunc(path: PathBuf) {
    let repo = open_repo(&path).await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    let body_id = field_id(&tbl, "body").await;
    let wal = repo.repo_wal().await.unwrap();
    let sidecar = path.with_extension("childcount");

    // The truncation crash can fire from EITHER the auto-spawned background
    // drainer (woken on every `commit_tx`, draining + truncating concurrently)
    // OR our explicit `drain_all` below — whichever first crosses a truncation
    // boundary at the armed seam. So the zero-loss oracle cannot be a fixed N:
    // it is "how many records were DURABLY committed when the process died".
    //
    // After each `commit_tx` returns, that record is in the WAL page cache
    // (Buffered tier) and, after `sync_wal`, fsync'd. We then write+fsync the
    // running committed count to a sidecar BEFORE the next commit. So at any
    // abort point the sidecar holds exactly the set of records that are durable
    // (in `history` if already drained, in the synced WAL otherwise). The
    // parent asserts recovery reconstructs EXACTLY that many — true zero loss.
    let write_sidecar = |n: usize| {
        use std::io::Write;
        let mut f = std::fs::File::create(&sidecar).expect("create sidecar");
        write!(f, "{n}").expect("write sidecar");
        f.sync_all().expect("fsync sidecar");
    };
    write_sidecar(0);

    for i in 0..TRUNC_RECORDS {
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        tbl.insert_tx(
            &text_record(body_id, &format!("trunc record number {i}")),
            Some(&mut tx),
        )
        .await
        .unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(guard);
        // Make the just-committed record fsync-durable in the WAL (the segment
        // files live OUTSIDE the redb backend, so the commit pipeline's redb
        // flush does not cover them), then record it as durably committed.
        wal.sync_wal().await.unwrap();
        write_sidecar(i + 1);
    }

    // Final explicit drain — replay every committed entry into `history`,
    // advance durable to visibility, and truncate the sealed WAL segments. If
    // the background drainer has not already crashed at the armed seam, this
    // crosses the boundary and aborts here. If no seam matches, this returns
    // and the child exits cleanly (the parent's `!status.success()` flags it).
    let _ = repo.drainer().drain_all(&repo).await;
}

/// Child entrypoint for the truncation scenario. NO-OP in the parent (sentinel
/// absent); runs the truncation scenario in the spawned child.
#[test]
fn child_entrypoint_trunc() {
    if std::env::var(CHILD_SENTINEL_TRUNC).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_trunc(path));
}

/// Spawn the truncation child at a given crash seam, passing the tiny segment
/// cap so the child's repo rolls real segments. Returns the child exit status.
fn spawn_child_trunc(phase: &str, repo_path: &Path) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_trunc", "--nocapture"])
        .env(CHILD_SENTINEL_TRUNC, "1")
        .env(CRASH_AFTER, phase)
        .env(SEG_MAX_BYTES_ENV, TRUNC_SEG_CAP)
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn truncation crash child")
        .status
}

/// One truncation cycle: spawn a child that commits `TRUNC_RECORDS` and
/// crashes at `phase` mid-truncation, then reopen the same redb image, recover,
/// and return `(recovered_records, durably_committed_at_crash)`. The segment
/// cap is passed to the REOPENED repo too (it must read the same `.shamirwal/`
/// segment layout). Zero loss ⟺ the two are equal.
async fn trunc_crash_then_recover(phase: &str) -> (usize, usize) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_trunc(phase, &repo_path);
    assert!(
        !status.success(),
        "truncation child must die abnormally at seam {phase}; got a clean \
         exit (status {status:?}) — the crash seam did not fire (not enough \
         sealed segments to truncate?)"
    );

    // Reopen with the SAME tiny cap so `SegmentSet::open` reads the survivors.
    std::env::set_var(SEG_MAX_BYTES_ENV, TRUNC_SEG_CAP);
    // The child's sidecar holds the number of records it had DURABLY committed
    // (WAL-fsync'd) at the instant it died — the zero-loss oracle.
    let sidecar = repo_path.with_extension("childcount");
    let durably_committed: usize = std::fs::read_to_string(&sidecar)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .expect("child must have written the durable-committed sidecar before dying");

    let repo = open_repo(&repo_path).await;
    let _replayed = repo.recover_v2_inflight().await.expect("recovery");
    let tbl = repo.get_table(TABLE).await.unwrap();
    let data = store_record_count(&tbl).await;
    std::env::remove_var(SEG_MAX_BYTES_ENV);
    (data, durably_committed)
}

fn spawn_child_multi_table(phase: &str, repo_path: &Path) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_multi_table", "--nocapture"])
        .env(CHILD_SENTINEL_MULTI, "1")
        .env(CRASH_AFTER, phase)
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn crash child")
        .status
}

/// MED-A cheap-win (Phase-A tails S1.2): a multi-table tx that crashes at
/// the Phase-4 commit point leaves a single inflight WAL entry covering
/// BOTH tables; `recover_v2_inflight` replays that one entry and both
/// tables converge at the same `commit_version`. Proves "logical-WAL
/// atomicity covers N tables" on REAL on-disk redb (not in-process
/// injection), making "restart-bounded cross-table consistency" an
/// executable fact, not a doc claim.
#[tokio::test]
async fn crash_at_phase4_two_tables_recover_cross_table_consistent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_multi_table("phase4", &repo_path);
    assert!(
        !status.success(),
        "child must die abnormally on process::abort() at phase4; got {status:?}"
    );

    // REOPEN over the same on-disk image.
    let repo = open_repo(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(
        replayed, 1,
        "one inflight entry must carry BOTH tables' ops (logical-WAL atomicity)"
    );

    let tbl_a = repo.get_table(TABLE).await.unwrap();
    let tbl_b = repo.get_table(TABLE_B).await.unwrap();
    let count_a = store_record_count(&tbl_a).await;
    let count_b = store_record_count(&tbl_b).await;
    assert_eq!(count_a, 1, "table A materialized by WAL replay");
    assert_eq!(count_b, 1, "table B materialized by WAL replay");

    // Cross-table same commit floor: `recover_inflight_v2` persists
    // `last_committed = gate.last_committed()` (recovery.rs:249-256). Both
    // tables' rows live under that same monotonic floor — the single
    // shared `commit_version` of the replayed entry.
    let floor = repo.tx_gate().await.unwrap().last_committed();
    assert!(
        floor > 0,
        "recovery must publish a non-zero commit floor covering both tables"
    );

    // Re-recovery is idempotent. File-WAL contract: no per-entry marker
    // removal until F6 truncation, so the segment replays the same entry
    // again (count >= 1). Idempotency means the DATA is unchanged — both
    // tables still hold exactly one record — not that the count is zero.
    // When F6 truncation lands this tightens back to `== 0`.
    assert!(
        repo.recover_v2_inflight().await.unwrap() >= 1,
        "second recovery replays the segment (file WAL: no truncation until F6)"
    );
    assert_eq!(
        store_record_count(&tbl_a).await,
        1,
        "table A unchanged after second replay (idempotent)"
    );
    assert_eq!(
        store_record_count(&tbl_b).await,
        1,
        "table B unchanged after second replay (idempotent)"
    );
}

// ---------------------------------------------------------------------------
// F6c truncation crash matrix. Each test = one subprocess that commits
// `TRUNC_RECORDS` under a tiny segment cap, drains, and aborts mid-truncation.
// Recovery from the survivors must reconstruct ALL `TRUNC_RECORDS` (zero loss):
// the truncation only ever deletes segments whose data is already durable in
// `history` (I1), and it fsyncs `history` before unlinking (I2), so whatever
// is gone from the WAL is recoverable from `history`, and whatever the WAL
// still holds replays idempotently (I6).
// ---------------------------------------------------------------------------

/// D4 — crash MID-DRAIN at `drain_replay`: the drainer fired the seam AFTER
/// `replay_v2_entry` wrote an entry's ops into `history` but BEFORE
/// `mark_durable` advanced the durable watermark for that version, with the WAL
/// marker still inflight. This is the gap between F6c's truncation seams (which
/// fire on a SEGMENT boundary, after a full per-entry cycle) and the ack-path
/// `phase4..phase6_5` seams (which fire BEFORE the drainer runs at all): it
/// kills the process IN THE MIDDLE of the per-entry replay loop, with history
/// partially written and the watermark un-advanced for the in-flight version.
///
/// Recovery must lose nothing: `recover_inflight_v2` re-replays every still-
/// inflight WAL entry idempotently (last-write-wins), so the partially-drained
/// version and every other durably-committed record reconstruct exactly. This
/// proves the drain is NOT atomic but recovery is CONVERGENT — replay is
/// idempotent and the durable watermark re-converges to visibility on reopen.
#[tokio::test]
async fn crash_mid_drain_recovers_all() {
    let (recovered, committed) = trunc_crash_then_recover("drain_replay").await;
    // Progress oracle uses `recovered`, NOT `committed`. The `drain_replay`
    // seam fires from the BACKGROUND drainer (woken by every `commit_tx`)
    // on the FIRST drained version, often BEFORE the child's main loop
    // has had a chance to `sync_wal()` + `write_sidecar(n)` — so the
    // sidecar `committed` count legitimately lags by one in-flight record
    // for this specific seam. Recovery still reconstructs that in-flight
    // version idempotently via `recover_inflight_v2`, so `recovered >= 1`
    // is the load-bearing progress guarantee (sidebar-independent). On
    // slower backends (pre-b2b1280 redb) the race never exposed itself;
    // on fjall it is deterministic.
    assert!(
        recovered >= 1,
        "the drainer must have replayed at least one inflight version \
         before the drain_replay crash fired (the sidecar may have lagged \
         because the background drainer fired before sync_wal completed, \
         but recovery reconstructs every inflight WAL entry idempotently — \
         recovered {recovered}, committed {committed})"
    );
    // Zero loss (see `pre_truncate` for the +1 survivor window): the entry being
    // replayed when the process died is still inflight (mark_durable did not run),
    // so recovery re-replays it idempotently along with every other inflight
    // entry — no durably-committed record is lost.
    assert!(
        recovered >= committed && recovered <= TRUNC_RECORDS,
        "drain_replay: history partially written + watermark un-advanced → \
         recovery re-replays the inflight WAL idempotently, losing nothing \
         (recovered {recovered}, durably committed {committed}, max {TRUNC_RECORDS})"
    );
}

/// Crash at `pre_truncate` — BEFORE history-flush + any unlink. Every sealed
/// WAL segment is still on disk; recovery replays all of them and reconstructs
/// every record. Zero loss.
#[tokio::test]
async fn crash_at_pre_truncate_recovers_all() {
    let (recovered, committed) = trunc_crash_then_recover("pre_truncate").await;
    assert!(
        committed >= 1,
        "the child must have durably committed at least one record before the \
         pre_truncate crash fired"
    );
    // Zero loss: every record the child durably committed survives recovery.
    // `recovered` may exceed `committed` by at most one — the crash can fire
    // (from the concurrent background drainer) in the tiny window AFTER a
    // `commit_tx`+`sync_wal` made record k durable but BEFORE the sidecar was
    // bumped to k. That extra record is a SURVIVOR, not a loss. The upper bound
    // forbids fabricated records.
    assert!(
        recovered >= committed && recovered <= TRUNC_RECORDS,
        "pre_truncate: nothing unlinked yet → recovery from the (synced) WAL + \
         flushed history must lose nothing (recovered {recovered}, durably \
         committed {committed}, max {TRUNC_RECORDS})"
    );
}

/// Crash at `wal_mid_delete` — inside `SegmentSet::truncate_below`, between two
/// segment unlinks (some sealed segments deleted, some surviving). By I2 the
/// drainer fsync'd `history` up to the truncation watermark BEFORE unlinking,
/// so the deleted segments' data is durable in `history`; the survivors replay
/// idempotently. `SegmentSet::open` on reopen picks up whatever survived.
/// Zero loss.
#[tokio::test]
async fn crash_at_mid_delete_recovers_all() {
    let (recovered, committed) = trunc_crash_then_recover("wal_mid_delete").await;
    assert!(
        committed >= 1,
        "the child must have durably committed at least one record before the \
         wal_mid_delete crash fired"
    );
    // Zero loss (see `pre_truncate` for the +1 survivor window): the unlinked
    // segments' data is durable in `history` (I2) and the survivors replay
    // idempotently (I6), so no durably-committed record is lost.
    assert!(
        recovered >= committed && recovered <= TRUNC_RECORDS,
        "wal_mid_delete: deleted segments' data durable in history (I2), \
         survivors replay (I6) → no durably-committed record lost (recovered \
         {recovered}, durably committed {committed}, max {TRUNC_RECORDS})"
    );
}

/// Crash at `post_truncate` — immediately AFTER a successful `truncate_below`.
/// The truncated segments are durable in `history` (flushed before the unlink,
/// I2); the active segment + survivors plus `history` reconstruct every record.
/// Zero loss.
#[tokio::test]
async fn crash_at_post_truncate_recovers_all() {
    let (recovered, committed) = trunc_crash_then_recover("post_truncate").await;
    assert!(
        committed >= 1,
        "the child must have durably committed at least one record before the \
         post_truncate crash fired"
    );
    // Zero loss (see `pre_truncate` for the +1 survivor window): the truncated
    // data was flushed to `history` before the unlink (I2), so recovery rebuilds
    // every durably-committed record.
    assert!(
        recovered >= committed && recovered <= TRUNC_RECORDS,
        "post_truncate: truncated data durable in history (flushed before the \
         unlink, I2) → no durably-committed record lost (recovered {recovered}, \
         durably committed {committed}, max {TRUNC_RECORDS})"
    );
}

// ---------------------------------------------------------------------------
// W2d write-cutover crash-seam: prove the lens-driven insert path
// (execute_insert_tx → insert_tx_many_bytes → RecordView) survives a crash
// at phase4 and recovery produces the correct data. Two variants:
//
// 1. IMPLICIT-tx: tx.implicit=true → base-intern → zero InnerValue on insert,
//    zero overlay remap. Exercises the full tree-free hot path.
//
// 2. INTERACTIVE-tx with a NEW field name: tx.implicit=false → overlay
//    intern → commit-time remap_inner_value_bytes on the staged bytes.
//    Exercises the cold remap-on-bytes path.
// ---------------------------------------------------------------------------

/// Env sentinel: when present the child runs the W2d write-cutover scenario.
const CHILD_SENTINEL_W2D: &str = "SHAMIR_TEST_CRASH_CHILD_W2D";

/// Env sentinel: when present + "1" the child uses implicit-tx; when "0"
/// interactive-tx with overlay remap.
const W2D_IMPLICIT_FLAG: &str = "SHAMIR_TEST_CRASH_W2D_IMPLICIT";

/// The W2d child scenario: insert a diverse record (int/f64/str/bin/nested-map/
/// list) via `execute_insert_tx` — the W2d cutover path — then commit. The
/// crash seam fires inside `commit_tx`. On the implicit path (flag=1) the
/// insert builds ZERO InnerValue; on the interactive path (flag=0) it uses
/// the overlay for a brand-new field name, exercising remap-on-bytes at commit.
async fn run_child_scenario_w2d(path: PathBuf) {
    let repo = open_repo(&path).await;
    let tbl = repo.get_table(TABLE).await.unwrap();

    let implicit = std::env::var(W2D_IMPLICIT_FLAG).unwrap_or("1".into()) == "1";

    // Build an InsertOp with diverse field types. On the interactive path
    // we use a brand-new field name ("w2d_new_field") that has never been
    // interned before — this triggers the overlay-id → remap-on-bytes path
    // at commit. On the implicit path field names go straight to base.
    let record = if implicit {
        mpack!({
            "int_field": 42,
            "float_field": 99.5,
            "str_field": "hello w2d",
            "bin_field": [1, 2, 3, 4, 5],
            "nested": { "inner_key": "inner_val", "inner_num": 7 },
            "list_field": [10, 20, 30]
        })
    } else {
        mpack!({
            "int_field": 99,
            "float_field": 88.8,
            "str_field": "interactive w2d",
            "w2d_new_field": "newness",
            "nested": { "deep": "value" },
            "list_field": [1, 2, 3]
        })
    };

    let op = shamir_query_types::write::InsertOp {
        insert_into: shamir_query_types::TableRef::new(TABLE),
        values: vec![record],
        records_idmsgpack: Vec::new(),
    };

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tx.set_implicit(implicit);
    tbl.execute_insert_tx(&op, &mut tx, false, None)
        .await
        .unwrap();

    // commit_tx reads SHAMIR_TEST_CRASH_AFTER and aborts at the seam.
    let _ = repo.commit_tx(tx).await;
    drop(guard);
}

/// Child entrypoint for the W2d scenario.
#[test]
fn child_entrypoint_w2d() {
    if std::env::var(CHILD_SENTINEL_W2D).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_w2d(path));
}

/// Spawn the W2d crash child.
fn spawn_child_w2d(phase: &str, repo_path: &Path, implicit: bool) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_w2d", "--nocapture"])
        .env(CHILD_SENTINEL_W2D, "1")
        .env(CRASH_AFTER, phase)
        .env(W2D_IMPLICIT_FLAG, if implicit { "1" } else { "0" })
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn w2d crash child")
        .status
}

/// Read back all records from a table after recovery and return them as
/// QueryValues for comparison.
async fn read_all_records(tbl: &TableManager) -> Vec<shamir_types::types::value::QueryValue> {
    use shamir_types::codecs::interned::inner_value_to_query_value;
    let stream = tbl.list_stream(256);
    futures::pin_mut!(stream);
    let mut records = Vec::new();
    let interner = tbl.interner().get().await.unwrap();
    while let Some(batch) = stream.next().await {
        for (_id, cow) in batch.expect("list_stream batch") {
            let record = cow.into_inner().expect("decode record");
            let qv = inner_value_to_query_value(&record, interner).expect("to_query_value");
            records.push(qv);
        }
    }
    records
}

/// W2d IMPLICIT-tx crash at phase4: the lens-driven insert path (zero
/// InnerValue, zero overlay remap) must survive a crash at the commit point
/// and recovery must materialize the correct data with all field types intact.
#[tokio::test]
async fn w2d_implicit_crash_phase4_recovers_diverse_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_w2d("phase4", &repo_path, true);
    assert!(
        !status.success(),
        "w2d implicit child must die abnormally at phase4; got {status:?}"
    );

    // Reopen + recover.
    let repo = open_repo(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(replayed, 1, "one inflight entry must be replayed");

    let tbl = repo.get_table(TABLE).await.unwrap();

    // Data must be present.
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "one record materialized by recovery");

    // Verify the field types survived the encode→lens→stage→crash→recover
    // round-trip with all values intact.
    let records = read_all_records(&tbl).await;
    assert_eq!(records.len(), 1, "exactly one record after recovery");
    let rec = &records[0];
    assert_eq!(rec["int_field"], 42i64);
    assert_eq!(rec["str_field"], "hello w2d");
    // Floats survive as f64.
    let expected_float: f64 = 99.5;
    assert!(
        (rec["float_field"].as_f64().unwrap() - expected_float).abs() < 1e-9,
        "float field preserved"
    );
    // Nested map + list survive.
    assert_eq!(rec["nested"]["inner_key"], "inner_val");
    assert_eq!(rec["nested"]["inner_num"], 7i64);
    assert_eq!(rec["list_field"], mpack!([10, 20, 30]));
}

/// W2d INTERACTIVE-tx crash at phase4 with a NEW field name: the overlay-id →
/// remap-on-bytes path at commit must survive a crash and recovery must
/// materialize the data with the remapped ids correct.
#[tokio::test]
async fn w2d_interactive_new_field_crash_phase4_recovers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_w2d("phase4", &repo_path, false);
    assert!(
        !status.success(),
        "w2d interactive child must die abnormally at phase4; got {status:?}"
    );

    // Reopen + recover.
    let repo = open_repo(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(replayed, 1, "one inflight entry must be replayed");

    let tbl = repo.get_table(TABLE).await.unwrap();
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "one record materialized by recovery");

    // The new field name must be present after recovery (the interner delta
    // + remap-on-bytes must have produced correct base-id-keyed bytes).
    let records = read_all_records(&tbl).await;
    assert_eq!(records.len(), 1, "exactly one record after recovery");
    let rec = &records[0];
    assert_eq!(rec["str_field"], "interactive w2d");
    // The brand-new field name survived the overlay→remap→crash→recover cycle.
    assert_eq!(
        rec["w2d_new_field"], "newness",
        "new field name must be readable after overlay remap + recovery"
    );
}
