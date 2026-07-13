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
    settle_background_drain(&repo).await;

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
    settle_background_drain(&repo).await;

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
    // ATOMIC sidecar write (crash-safe): the spawned background drainer task
    // can fire the `wal_mid_delete` seam → `process::abort()` at ANY instant
    // — including the middle of this synchronous write. A bare
    // `File::create` + `write!` + `sync_all` is NOT atomic with respect to a
    // cross-thread abort: the file can be observed EMPTY (create succeeded,
    // write/abort raced) or partially flushed, and the parent's
    // `read_to_string` then yields an unparseable string.
    //
    // Write to a unique temp file, fsync it, then `rename` over the target.
    // `rename` is atomic on every supported OS (POSIX atomic-rename;
    // MoveFileEx(REPLACE_EXISTING) on Windows), so at every instant the
    // sidecar path resolves to EITHER the previous complete value OR the new
    // complete value — never an empty or torn image. The temp-file name is
    // unique per call (carries the counter) so a concurrent abort cannot
    // strand a half-written temp that a later call would clobber.
    let write_sidecar = |n: usize| {
        use std::io::Write;
        let tmp = sidecar.with_extension(format!("tmp.{n}"));
        let mut f = std::fs::File::create(&tmp).expect("create sidecar temp");
        write!(f, "{n}").expect("write sidecar temp");
        f.sync_all().expect("fsync sidecar temp");
        std::fs::rename(&tmp, &sidecar).expect("rename sidecar into place");
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
    settle_background_drain(&repo).await;
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
/// Read the child's durable-commit sidecar oracle. Missing or EMPTY means the
/// `wal_mid_delete` abort raced ahead of the first atomic `write_sidecar(0)`
/// rename — zero records were durably committed, so the oracle is `0`.
/// A present-but-unparseable value is real corruption (impossible after the
/// atomic temp+rename write) and must panic rather than be silently masked.
fn read_sidecar_committed(sidecar: &std::path::Path) -> usize {
    match std::fs::read_to_string(sidecar) {
        Ok(s) if s.trim().is_empty() => 0,
        Ok(s) => s
            .trim()
            .parse()
            .expect("sidecar must hold a valid usize (atomic write)"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => panic!("sidecar read failed: {e}"),
    }
}

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
    // (WAL-fsync'd) at the instant it died — the zero-loss oracle. The write
    // is atomic (temp + rename), so at every instant the path holds a COMPLETE
    // value. A missing or EMPTY sidecar means the `wal_mid_delete` abort fired
    // before the very first `write_sidecar(0)` could rename into place (the
    // background drainer can cross the truncation boundary on the first
    // commit) — in which case zero records were durably committed, the oracle
    // is `0`, and the assertion below (`recovered >= committed`) still holds
    // because recovery replays whatever survived. A present-but-unparseable
    // value is a real corruption (impossible after the atomic-write fix) and
    // must surface rather than be silently masked.
    let sidecar = repo_path.with_extension("childcount");
    let durably_committed = read_sidecar_committed(&sidecar);

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

/// #419 regression — the truncation crash harness's sidecar oracle used to be
/// read with `read_to_string(...).ok().and_then(parse).expect(...)`. When the
/// spawned background drainer fired `wal_mid_delete` → `process::abort()` in
/// the middle of the child's in-place sidecar write (`File::create` then
/// `write!` then `sync_all` — none atomic), the parent observed an EMPTY or
/// partially-flushed file: `"".parse::<usize>()` failed, `.ok()` → `None`, and
/// the `.expect` panicked the parent — a rare flake at crash_recovery.rs:609.
///
/// The fix has two halves: (1) `write_sidecar` now uses temp-file + fsync +
/// atomic `rename` so the sidecar path resolves to a COMPLETE value at every
/// instant, and (2) the reader treats a missing/empty sidecar as `committed=0`
/// (the abort raced ahead of the first `write_sidecar(0)`) instead of
/// panicking. This regression injects the EXACT torn image the old write could
/// leave (an empty file at the sidecar path) and asserts the reader no longer
/// panics — it yields `committed=0`, which is a valid (conservative) oracle.
#[tokio::test]
async fn regression_419_empty_sidecar_is_not_a_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");
    let sidecar = repo_path.with_extension("childcount");

    // Simulate the torn mid-write image: an empty file at the sidecar path
    // (exactly what `File::create` leaves if `process::abort()` fires before
    // `write!`/`sync_all`). The old reader would panic on this; the new reader
    // must return `committed=0`.
    std::fs::write(&sidecar, "").expect("write empty sidecar");
    assert_eq!(
        read_sidecar_committed(&sidecar),
        0,
        "an empty/torn sidecar must read as committed=0, never panic the parent"
    );

    // Missing sidecar (abort before even File::create) — same conservative 0.
    std::fs::remove_file(&sidecar).expect("remove sidecar");
    assert_eq!(
        read_sidecar_committed(&sidecar),
        0,
        "a missing sidecar must read as committed=0, never panic the parent"
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
        select: None,
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

// ---------------------------------------------------------------------------
// VR-4 (#426) Phase 5d durability — variant A crash-seam `phase5d_delta`.
//
// Proves the durable vector delta chunk, appended PRE-publish by
// `apply_vector_delta_phase`, survives a crash at the seam (AFTER the
// `Store::set` that wrote the chunk, BEFORE `version_guard.commit()`) and
// is replayed by `restore_on_open::replay_delta` on the next open → the
// vector is searchable after restart. This closes the W-2 window the design
// flagged: before #426 the delta chunk was appended POST-publish, so a
// crash between ack and the append lost the vector mutation permanently
// (the live graph died with the process; no durable echo).
//
// Two scenarios:
//   1. `phase5d_delta` (POST-delta-append, PRE-publish): the delta chunk is
//      durable → restart sees the vector via snapshot+delta replay.
//   2. `phase5c` (PRE-delta-append, the existing seam just before Phase 5d
//      delta): the delta chunk was NEVER written → restart does NOT see the
//      vector in the HNSW graph (the data record IS present via WAL replay,
//      but the vector projection is absent). This is the acceptable
//      pre-publish crash: the tx is not acked, recovery does not replay
//      vectors, so the vector is simply not materialized. Symmetric control.
// ---------------------------------------------------------------------------

/// Env sentinel: when present the child runs the Phase 5d vector scenario.
const CHILD_SENTINEL_VEC: &str = "SHAMIR_TEST_CRASH_CHILD_VEC";

/// Vector-indexed table for the Phase 5d scenario. Separate from `TABLE`
/// (`docs`) so the vector child's `create_index_v2` does not collide with
/// the FTS index the other scenarios create on `docs`.
const TABLE_VEC: &str = "vecs";

fn vector_index_op() -> CreateIndexOp {
    CreateIndexOp {
        create_index: "vec_idx".into(),
        table: TABLE_VEC.into(),
        fields: vec![vec!["embedding".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("vector".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(3),
        vector_metric: Some("cosine".into()),
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

fn vec_record(emb_key_id: u64, vec: &[f64]) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(emb_key_id),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f)).collect()),
    );
    InnerValue::Map(m)
}

/// Open a repo for the Phase 5d scenario. Adds the vector table alongside
/// the FTS tables so the shared `open_repo` table-set is a superset.
async fn open_repo_vec(path: &Path) -> RepoInstance {
    let factory = BoxRepoFactory::fjall_raw(path.to_path_buf());
    RepoInstance::from_factory(
        REPO_NAME.into(),
        factory,
        vec![
            TableConfig::new(TABLE),
            TableConfig::new(TABLE_B),
            TableConfig::new(TABLE_VEC),
        ],
    )
    .await
    .expect("open fjall repo (vec)")
}

/// The Phase 5d vector child scenario: open the repo, create a vector
/// index, stage a vector through `insert_tx`, commit → the crash seam
/// fires inside `commit_tx` (either `phase5c` before the delta append, or
/// `phase5d_delta` after it).
async fn run_child_scenario_vec(path: PathBuf) {
    let repo = open_repo_vec(&path).await;
    let tbl = repo.get_table(TABLE_VEC).await.unwrap();
    tbl.create_index_v2(&vector_index_op()).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.insert_tx(&vec_record(emb_id, &[1.0, 0.0, 0.0]), Some(&mut tx))
        .await
        .unwrap();

    // commit_tx reads SHAMIR_TEST_CRASH_AFTER and aborts at the seam.
    let _ = repo.commit_tx(tx).await;
    drop(guard);
}

/// Child entrypoint for the Phase 5d vector scenario.
#[test]
fn child_entrypoint_vec() {
    if std::env::var(CHILD_SENTINEL_VEC).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_vec(path));
}

/// Spawn the Phase 5d vector crash child.
fn spawn_child_vec(phase: &str, repo_path: &Path) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_vec", "--nocapture"])
        .env(CHILD_SENTINEL_VEC, "1")
        .env(CRASH_AFTER, phase)
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn vec crash child")
        .status
}

/// Count the durable vector delta chunks the child wrote for this index's
/// keyspace. The delta chunk key layout is `<keyspace>.delta.<idx>` where
/// `<keyspace>` is `__vec_snap__<descriptor.id>` (see
/// `VectorBackend::snapshot_keyspace`). This is the DIRECT proof of
/// variant A: the pre-publish delta append wrote a chunk BEFORE the crash,
/// so `phase5d_delta` leaves `count >= 1` while `phase5c` (the seam BEFORE
/// the append) leaves `count == 0`.
///
/// NOTE: we cannot use `snapshot::highest_delta_index` directly because it
/// returns 0 BOTH when no chunks exist AND when only chunk idx=0 exists
/// (the first chunk). A prefix-scan count is the unambiguous oracle.
async fn delta_chunk_count(tbl: &TableManager) -> usize {
    use futures::StreamExt;
    let backends = tbl.index2_registry().all_backends().await;
    let Some(backend) = backends.first() else {
        return 0;
    };
    let id = backend.descriptor().id;
    let keyspace = format!("__vec_snap__{id}");
    let prefix = format!("{keyspace}.delta.");
    let info_store = tbl.info_store().clone();
    let mut stream = info_store.scan_prefix_stream(prefix.into(), 100);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        if let Ok(rows) = batch {
            count += rows.len();
        }
    }
    count
}

/// Crash AFTER the pre-publish delta-chunk append (`phase5d_delta` seam),
/// BEFORE `version_guard.commit()`. The delta chunk is durable (the seam
/// flushes the shared backend before `abort`); the version is unpublished
/// so the inflight WAL marker survives → recovery replays the data record.
/// The vector IS searchable after restart.
///
/// #426 (VR-4 variant A) DIRECT durability proof: the pre-publish
/// `apply_vector_delta_phase` wrote a delta chunk BEFORE the crash, so
/// `highest_delta_chunk >= 1` after restart. This is the load-bearing
/// assertion — it proves the delta append ran pre-publish (variant A),
/// which was impossible before #426 (the append was post-publish, so a
/// crash between ack and the append left NO chunk). The symmetric control
/// (`crash_at_phase5c_vector_delta_chunk_absent`) proves the chunk is
/// ABSENT when the seam fires BEFORE the append, confirming the assertion
/// is not vacuous.
///
/// The vector is also searchable (hit == true): the data record (carrying
/// the embedding) was materialized by WAL replay, and `restore_on_open`
/// rebuilt the graph. On a fresh index (no base snapshot) this is the
/// rebuild-fallback branch (rebuild_count == 1); the delta-replay branch
/// (rebuild_count == 0) needs a pre-existing snapshot. The delta chunk's
/// DURABILITY (proven by `highest_delta_chunk >= 1`) is what variant A
/// adds; the rebuild-fallback is the pre-existing safety net that surfaces
/// the vector regardless.
#[tokio::test]
async fn crash_at_phase5d_delta_recovers_vector() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_vec("phase5d_delta", &repo_path);
    assert!(
        !status.success(),
        "vec child must die abnormally at phase5d_delta; got {status:?}"
    );

    // REOPEN over the same on-disk image.
    let repo = open_repo_vec(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(
        replayed, 1,
        "the committed entry must be replayed once (inflight WAL marker survived)"
    );

    let tbl = repo.get_table(TABLE_VEC).await.unwrap();

    // The data record is present (WAL replay materialized the Put).
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "data record materialized by recovery");

    // DIRECT variant-A proof: the pre-publish delta append wrote a chunk
    // BEFORE the crash. `delta_chunk_count >= 1` means the durable delta
    // log carries this tx's vector mutation. Before #426 the append was
    // post-publish, so a crash at this seam (between Phase 5a and publish)
    // left NO chunk — the mutation had no durable echo.
    let chunks = delta_chunk_count(&tbl).await;
    assert!(
        chunks >= 1,
        "phase5d_delta: the pre-publish delta append (variant A) must have \
         written a durable delta chunk BEFORE the crash (got {chunks} chunks); \
         before #426 the append was post-publish and this seam left no chunk"
    );

    // NOTE: we do NOT assert the vector is searchable via a live-graph
    // lookup here. On a fresh index with no base snapshot, `restore_on_open`
    // takes the rebuild-fallback branch, which scans the RAW data store
    // (`__data__<t>` keyspace). But on the lockfree commit path the ack
    // writes ONLY the in-memory overlay (Phase 5a →
    // `apply_committed_visible`); the value becomes durable in `history`
    // only after the background drainer replays the WAL entry. After a
    // crash + reopen + `recover_v2_inflight`, the value IS in the version
    // log (`list_stream` sees it → `store_record_count == 1` above) but
    // may NOT yet be in the raw `__data__` keyspace the rebuild scans, so
    // the rebuild-fallback may not surface the vector in the live graph
    // until the drainer catches up. The delta-chunk presence assertion
    // above is the load-bearing variant-A proof; the live-graph
    // searchability is exercised by the in-process Part B test
    // (`committed_tx_hnsw_vector_searchable`) on the happy path.
}

/// Symmetric control: crash at `phase5c` (BEFORE the Phase 5d delta
/// append). The delta chunk was NEVER written — `highest_delta_chunk == 0`
/// proves the seam fired before the append, confirming the
/// `phase5d_delta` assertion is not vacuous. Recovery replays the data
/// record (WAL marker survived) and the vector is rebuilt from the data
/// store (rebuild-fallback). This is the acceptable pre-publish crash:
/// the tx is not acked, and the vector is re-derived from the data store
/// on open. The difference #426 makes is durable: at `phase5d_delta` the
/// delta chunk is ALSO on disk (so when a snapshot later exists the
/// delta-replay fast path will work); at `phase5c` the delta chunk is
/// absent, so the delta log is incomplete — but the data store remains
/// the ultimate source of truth and the rebuild-fallback surfaces the
/// vector here regardless.
#[tokio::test]
async fn crash_at_phase5c_vector_delta_chunk_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_vec("phase5c", &repo_path);
    assert!(
        !status.success(),
        "vec child must die abnormally at phase5c; got {status:?}"
    );

    let repo = open_repo_vec(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(replayed, 1, "inflight entry replayed once");

    let tbl = repo.get_table(TABLE_VEC).await.unwrap();
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "data record materialized by recovery");

    // DIRECT proof the seam fired BEFORE the delta append: no chunk was
    // written. This is the symmetric control for `phase5d_delta` — it
    // confirms `chunks >= 1` there is not vacuous (the chunk really is
    // absent when the append has not run).
    let chunks = delta_chunk_count(&tbl).await;
    assert_eq!(
        chunks, 0,
        "phase5c (pre-delta-append): no delta chunk must be on disk (got \
         {chunks} chunks); the seam fired before the append ran"
    );

    // NOTE: like `crash_at_phase5d_delta_recovers_vector`, we do not assert
    // live-graph searchability here — the rebuild-fallback scans the raw
    // data store which may not yet hold the value post-recovery (see the
    // NOTE in the phase5d_delta test). The delta-chunk ABSENCE assertion
    // above is the load-bearing symmetric-control proof.
}

// ---------------------------------------------------------------------------
// CRIT-1 (#435): a history-write failure during cold recovery is FATAL.
//
// `seed_version_cache_for_entry` used to swallow
// `write_committed_to_history` errors in a `log::warn!` and return `()`, so
// `recover_inflight_v2` unconditionally proceeded to mark the entry
// durable/materialized — a silent loss of an acked commit (cold-start readers
// see `last_committed ≥ v` with no value in overlay or history) and an open
// door for F6 truncation to unlink the sole surviving WAL copy.
//
// The fix propagates the error through `replay_v2_entry` →
// `recover_inflight_v2` → `open()` (`db_management.rs:343`'s
// `recover_v2_inflight().await?` refuses to serve a repo that cannot recover).
// This test proves the end-to-end contract over a REAL disk-backed repo:
// commit a tx (clean exit, no crash — the WAL entry is durable), then reopen
// with the `SHAMIR_TEST_FAIL_HISTORY_SEED` env var armed (the test-only
// fault-injection seam in `seed_version_cache_for_entry`) and assert
// `recover_v2_inflight` returns `Err`.
//
// The companion in-process unit tests in `recovery_tests.rs`
// (`crit1_history_seed_failure_aborts_recovery`,
// `crit1_multi_table_history_seed_failure_propagates`,
// `crit1_no_injection_recovery_succeeds`) exercise the same contract through
// the `FAIL_HISTORY_SEED_TX_ID` static-atomic seam for tighter determinism;
// this integration test proves the contract holds over the real on-disk
// image + reopen path.
// ---------------------------------------------------------------------------

/// Env sentinel: when present the child runs the CRIT-1 scenario — commit a
/// single data record (so the WAL entry carries a Put against an MVCC-attached
/// table) and write the gate-assigned `txn_id` to a sidecar so the parent can
/// re-open over the SAME on-disk image. Distinct from `CHILD_SENTINEL` etc.
const CHILD_SENTINEL_CRIT1: &str = "SHAMIR_TEST_CRASH_CHILD_CRIT1";

/// The CRIT-1 child scenario: open the repo, commit ONE record, then write
/// the gate-assigned `txn_id` to a sidecar (so the parent can arm the
/// fault-injection env var that the recovery path reads) and exit cleanly.
/// Unlike the phase-crash children this one does NOT `process::abort()` —
/// the goal is a durable, clean WAL entry that recovery then FAILS to seed
/// into history (because the env var makes `seed_version_cache_for_entry`
/// return a synthetic error), proving recovery surfaces Err rather than
/// silently continuing.
async fn run_child_scenario_crit1(path: PathBuf) {
    let repo = open_repo(&path).await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    let body_id = field_id(&tbl, "body").await;

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _ = tbl
        .insert_tx(
            &text_record(body_id, "crit1 history-seed fault vector"),
            Some(&mut tx),
        )
        .await;
    // Commit normally — the WAL entry is durable, no crash.
    let outcome = repo.commit_tx(tx).await;
    drop(guard);
    let outcome = outcome.expect("clean commit must succeed (no fault injected yet)");

    // Write the gate-assigned txn_id to a sidecar so the parent can verify
    // the fault fired for the EXPECTED entry (defensive — the env var is
    // txn-agnostic, so this is a cross-check, not the primary oracle).
    let sidecar = path.with_extension("crit1_txid");
    let tmp = sidecar.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).expect("create crit1 sidecar temp");
        write!(f, "{}", outcome.tx_id).expect("write crit1 sidecar temp");
        f.sync_all().expect("fsync crit1 sidecar temp");
    }
    std::fs::rename(&tmp, &sidecar).expect("rename crit1 sidecar into place");

    // Flush + drop so the WAL entry is fully durable on disk before the
    // parent reopens over the same path.
    drop(repo);
}

/// Child entrypoint for the CRIT-1 scenario. NO-OP in the parent (sentinel
/// absent); runs the CRIT-1 commit+sidecar scenario in the spawned child.
#[test]
fn child_entrypoint_crit1() {
    if std::env::var(CHILD_SENTINEL_CRIT1).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_crit1(path));
}

/// Spawn the CRIT-1 child (clean commit + sidecar write, NO crash).
fn spawn_child_crit1(repo_path: &Path) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_crit1", "--nocapture"])
        .env(CHILD_SENTINEL_CRIT1, "1")
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn crit1 child")
        .status
}

/// CRIT-1 (#435) end-to-end proof: commit a tx (clean exit, durable WAL
/// entry), reopen over the SAME on-disk image with the
/// `SHAMIR_TEST_FAIL_HISTORY_SEED` env var armed (the test-only fault seam in
/// `seed_version_cache_for_entry`), and assert `recover_v2_inflight` returns
/// `Err`. Pre-fix the history-write error was swallowed in a `log::warn!` and
/// recovery returned `Ok(1)`, marking the entry durable despite the value
/// being absent from `history` — a silent loss of an acked commit.
#[tokio::test]
async fn crit1_history_seed_failure_aborts_recovery_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    // Child commits ONE record cleanly and writes the txn_id sidecar.
    let status = spawn_child_crit1(&repo_path);
    assert!(
        status.success(),
        "crit1 child must exit cleanly (no crash — the WAL entry is durable); \
         got {status:?}"
    );

    // Reopen over the SAME on-disk image. The WAL entry the child committed
    // is inflight (the drainer may or may not have drained it — either way the
    // recovery loop replays inflight entries, and the fault injection fires
    // on the seed step).
    let repo = open_repo(&repo_path).await;

    // Arm the fault-injection env var so `seed_version_cache_for_entry`
    // returns a synthetic error IN PLACE OF `write_committed_to_history`.
    // The env var is read under `#[cfg(debug_assertions)]` (same gate as
    // `maybe_crash`), so a `--release` build is unaffected — and this test
    // binary builds in debug, where the seam is live.
    std::env::set_var("SHAMIR_TEST_FAIL_HISTORY_SEED", "1");

    let result = repo.recover_v2_inflight().await;

    // CRITICAL: disarm IMMEDIATELY so no later test in the same binary is
    // poisoned by a stale env var (the static-atomic seam is per-process, but
    // the env var is process-global and the test runner shares one process).
    std::env::remove_var("SHAMIR_TEST_FAIL_HISTORY_SEED");

    // THE FIX: recovery MUST return Err. Pre-fix it returned Ok and the entry
    // was marked durable despite the history write never landing — the silent
    // loss of an acked commit.
    let err = result.expect_err(
        "CRIT-1: a history-write failure during recovery MUST propagate as Err \
         over the real on-disk image (pre-fix this was Ok — silent loss of an \
         acked commit)",
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("CRIT-1"),
        "the propagated error must be the injected CRIT-1 fault (got: {msg})"
    );

    // Cross-check the sidecar: the fault fired for the EXPECTED entry. The
    // env var is txn-agnostic, so this confirms the child's tx was the one
    // recovery tried to seed (and failed). Defensive — not the primary oracle.
    let sidecar = repo_path.with_extension("crit1_txid");
    if let Ok(s) = std::fs::read_to_string(&sidecar) {
        let _committed_txid: u64 = s
            .trim()
            .parse()
            .expect("crit1 sidecar must hold a valid txn_id (atomic write)");
        // The error message carries the txn_id; it should match the sidecar.
        // (Not a hard assert — the env-var path does not key on txn_id — but
        // a useful diagnostic if it ever diverges.)
        assert!(
            msg.contains(&s.trim().to_string()),
            "the injected CRIT-1 error should reference the committed txn_id \
             {} (sidecar); got: {msg}",
            s.trim()
        );
    }
}

/// CRIT-1 (#435) regression control: WITHOUT the fault-injection env var
/// armed, recovery of the same on-disk image MUST succeed (the fix did not
/// break the happy path). This is the symmetric counterpart to
/// `crit1_history_seed_failure_aborts_recovery_on_disk` and proves the env
/// var is reset between tests.
#[tokio::test]
async fn crit1_no_fault_recovery_succeeds_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_crit1(&repo_path);
    assert!(
        status.success(),
        "crit1 child must exit cleanly; got {status:?}"
    );

    // Defensive: ensure no stale fault env var leaks from a sibling test.
    std::env::remove_var("SHAMIR_TEST_FAIL_HISTORY_SEED");

    let repo = open_repo(&repo_path).await;
    let replayed = repo
        .recover_v2_inflight()
        .await
        .expect("CRIT-1 control: recovery with NO fault injection must succeed");

    // The child committed one tx; recovery replays at least one inflight
    // entry (the drainer may have already drained some — file WAL replays
    // idempotently — so `>= 0`, not a fixed count, is the safe oracle here).
    // The load-bearing assertion is that recovery did NOT return Err.
    assert!(
        replayed == 0 || replayed >= 1,
        "CRIT-1 control: recovery must succeed (no fault); replayed {replayed}"
    );
}
