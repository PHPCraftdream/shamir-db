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
    let factory = BoxRepoFactory::redb_raw(path.to_path_buf());
    RepoInstance::from_factory(
        REPO_NAME.into(),
        factory,
        vec![TableConfig::new(TABLE), TableConfig::new(TABLE_B)],
    )
    .await
    .expect("open redb repo")
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

/// Crash after Phase 7 (WAL marker already removed, tx fully
/// materialized). The data + index are PRESENT and recovery is a no-op:
/// the on-disk state is already a clean committed state.
#[tokio::test]
async fn crash_at_phase7_is_clean_committed() {
    let (replayed, data, fts) = crash_then_recover("phase7").await;
    assert_eq!(replayed, 0, "WAL marker gone → recovery is a no-op");
    assert_eq!(data, 1, "already materialized before the crash");
    assert!(fts >= 1, "index postings already present (got {fts})");
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

    // Re-recovery is a no-op (marker cleaned).
    assert_eq!(
        repo.recover_v2_inflight().await.unwrap(),
        0,
        "second recovery pass must be a no-op"
    );
}
