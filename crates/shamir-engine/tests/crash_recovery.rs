#![cfg(feature = "fjall")]
//! Real crash-recovery harness — kill mid-commit at each phase, prove
//! atomicity around the Phase-4 WAL commit point (Vector II.1). Unlike
//! the in-process `drop(Arc<InMemoryRepo>)` "crash" tests, this
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
#[path = "clean_horizon/admitted_field_id.rs"]
mod admitted_field_id;
#[path = "crash_recovery/child_proc.rs"]
mod child_proc;
#[path = "clean_horizon/crash_matrix.rs"]
mod clean_horizon_crash_matrix;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use shamir_query_types::admin::CreateIndexOp;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::InternerKey;
use shamir_types::mpack;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use admitted_field_id::field_id;

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

/// Number of records the truncation child commits. Must be large enough that:
///
/// 1. The WAL seals MULTIPLE segments under `TRUNC_SEG_CAP` (so `wal_mid_delete`
///    has >= 2 truncatable segments to delete between). At ~186 bytes/record
///    (single-field map, bincode `WalEntryV2`) and a 4096-byte cap, a segment
///    seals roughly every 22 records, so 96 records seal ~4 segments.
///
/// 2. The commit count clears `INTERNER_CHECKPOINT_INTERVAL` (64). Each record
///    references the interned `"body"` field id, so every entry carries a
///    non-empty `interner_delta`. The drainer's A5 gate
///    (`interner_delta_safe_to_truncate`) blocks WAL truncation for an entry
///    until its delta's max id is `<= persisted_high_water()`. That high-water
///    mark only advances when the background interner checkpoint fires — which
///    happens at `commit_version % 64 == 0` (commit 32e63e17, "A5 — remove
///    interner.persist() from Phase 1 + checkpoint mechanism"). Below 64
///    commits the checkpoint NEVER fires, `persisted_high_water()` stays below
///    the `"body"` id, the A5 gate permanently pins the truncation ceiling at
///    0, `has_truncatable` is never true, and NONE of the truncation crash
///    seams ever fire — the child exits cleanly and the parent's
///    `!status.success()` assertion fails. 96 clears 64 with a 32-record
///    margin so the spawned checkpoint completes (a few await points) while
///    versions are still flowing through the drainer, guaranteeing a
///    `drain_step` with `drained > 0` re-runs `settle_and_truncate` after the
///    high-water mark advanced and the ceiling opens.
const TRUNC_RECORDS: usize = 96;

/// Tiny segment cap (bytes) for the truncation scenario. At ~186 bytes/record
/// a 4096-byte cap seals a segment roughly every 22 records, so `TRUNC_RECORDS`
/// (96) commits produce ~4 sealed segments for truncation to reclaim.
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
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
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
    let backends = tbl.index2_read_view().backends().await;
    let Some(backend) = backends.first() else {
        return 0;
    };
    match backend
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("atomicity")],
            mode: FtsMode::AndAll,
            limit: None,
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

/// Flake fix (found via a full `@engine --full` stress run): `commit_tx`'s
/// tail already calls `repo.drainer().wake()`, which lazily spawns AND wakes
/// the background drain loop — racing an explicit `drain_all()` call with no
/// single-flight guard between them (by design, see `Drainer::spawn`'s doc:
/// "single-owner contract... is the caller's responsibility", not enforced
/// here since both passes are individually idempotent). If the background
/// task wins partial progress on the entry (e.g. it suspends between
/// `gate.mark_durable` and the `phase7` `maybe_crash` check, both inside
/// `drain_step`) while an explicit `drain_all()` call finds nothing left to
/// drain and returns immediately, the child scenario would return,
/// `child_entrypoint`'s `rt.block_on` would return, and the tokio runtime
/// would be torn down — CANCELLING the still-in-flight background task
/// before it ever reaches the phase7 crash seam. Net effect: a clean exit
/// instead of the expected abort, observed as an intermittent
/// `crash_at_phase7_is_clean_committed` failure under full-suite scheduling
/// contention.
///
/// Fix: give the background task a bounded window to finish its OWN pass
/// (and hit the seam, if armed) before the scenario returns. `wal.recover()`
/// returning empty is a task-agnostic proxy for "fully drained" — true
/// regardless of which task (background or the explicit call) did the
/// draining. Bounded to 500ms total; a real single small entry drains in
/// microseconds, so this only ever adds latency in the pathological race
/// window, never on the common path.
async fn settle_background_drain(repo: &RepoInstance) {
    if let Ok(wal) = repo.repo_wal().await {
        for _ in 0..50 {
            match wal.recover().await {
                Ok(entries) if entries.is_empty() => break,
                _ => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
    }
}

/// The scenario the spawned child runs. On a successful return (only when
/// the crash seam did not match — e.g. an unknown phase label) the child
/// exits 0; normally the commit pipeline `process::abort()`s before this
/// returns, so the parent observes an abnormal exit.
async fn run_child_scenario(path: PathBuf) {
    eprintln!("crash-child: open");
    let repo = open_repo(&path).await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    eprintln!("crash-child: create_index");
    tbl.create_index_v2(&fts_index_op()).await.unwrap();

    let body_id = field_id(&repo, &tbl, "body").await;

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    eprintln!("crash-child: insert");
    tbl.insert_tx(
        &text_record(body_id, "shamir crash recovery atomicity"),
        Some(&mut tx),
    )
    .await
    .unwrap();

    // commit_tx will read SHAMIR_TEST_CRASH_AFTER and abort at the
    // matching seam. If the label is unknown it commits normally.
    eprintln!("crash-child: commit_tx");
    let result = repo.commit_tx(tx).await;
    eprintln!("crash-child: commit_tx returned {result:?}");

    // D2 P1d-2c: post-cutover the "phase7" seam (tx fully materialized + WAL
    // cleanup) lives in the background drainer, not the ack-path. Drive the
    // drainer so a `phase7` crash fires here. For the inline phases
    // (pre_commit..phase6_5) the process has already `process::abort`ed inside
    // `commit_tx` above and never reaches this line.
    let drainer = repo.drainer();
    let drain = drainer.drain_all(&repo);
    if tokio::time::timeout(Duration::from_secs(10), drain)
        .await
        .is_err()
    {
        eprintln!("crash-child: drain_all timed out");
        std::process::exit(child_proc::CHILD_STALL_EXIT);
    }
    settle_background_drain(&repo).await;

    drop(guard);
}

// ---------------------------------------------------------------------------
// PARENT: spawn child at a phase, then reopen + recover + assert.
// ---------------------------------------------------------------------------

/// Spawn the crash child: re-exec THIS test binary with the child
/// sentinel + crash phase + repo path set, running only the
/// `child_entrypoint` test (`--exact`). Returns the child's exit status.
async fn spawn_child(phase: &str, repo_path: &Path) -> (std::process::ExitStatus, String) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = tokio::process::Command::new(exe);
    // Run ONLY the child entrypoint test so the child does not
    // recurse into the parent harness tests.
    cmd.args(["--exact", "child_entrypoint", "--nocapture"])
        .env(CHILD_SENTINEL, "1")
        .env(CRASH_AFTER, phase)
        .env(REPO_PATH, repo_path);
    child_proc::run_crash_child(phase, cmd).await
}

/// One full cycle: spawn a child that crashes at `phase`, then reopen the
/// same redb file, recover, and return `(replayed_entries, data_records,
/// fts_hits)`.
async fn crash_then_recover(phase: &str) -> (usize, usize, usize) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let (status, child_output) = spawn_child(phase, &repo_path).await;
    assert_ne!(
        status.code(),
        Some(child_proc::CHILD_STALL_EXIT),
        "child exited via CHILD_STALL_EXIT at {phase}: {child_output}"
    );
    assert!(
        !status.success(),
        "child must die abnormally at {phase}: {status:?}\n{child_output}"
    );
    let stage = Cell::new("open_repo");
    tokio::time::timeout(Duration::from_secs(60), async {
        // === RESTART === reopen a fresh RepoInstance over the same on-disk
        // redb image the killed child left behind.
        let repo = open_repo(&repo_path).await;

        // Recover FIRST so the WAL replay writes any committed index postings
        // to info_store before we register a backend to read them.
        stage.set("recover_v2_inflight");
        let replayed = repo.recover_v2_inflight().await.expect("recovery");

        // `get_table` auto-loads the persisted index2 descriptors, so on the
        // committed phases the FTS backend is already registered (with the
        // child's original id) and reads the RECOVERED postings — no
        // re-indexing happens here.
        stage.set("get_table");
        let tbl = repo.get_table(TABLE).await.unwrap();
        stage.set("record counts");
        let data = store_record_count(&tbl).await;
        let fts = fts_hit_count(&tbl).await;

        // tempdir drops here — the child process is already dead so the file
        // is no longer open.
        (replayed, data, fts)
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "parent stalled in {} after {phase} crash\n{child_output}",
            stage.get()
        )
    })
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
    rt.shutdown_timeout(Duration::from_secs(5));
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

#[path = "crash_recovery/history_seed.rs"]
mod history_seed;
#[path = "crash_recovery/multi_table_drain.rs"]
mod multi_table_drain;
#[path = "crash_recovery/vector_delta.rs"]
mod vector_delta;
#[path = "crash_recovery/write_to_data.rs"]
mod write_to_data;
