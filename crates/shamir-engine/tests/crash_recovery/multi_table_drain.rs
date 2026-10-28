//! Split out of `tests/crash_recovery.rs` (layout sweep Q2); bodies moved verbatim,
//! root harness items reached via `super::`.

use super::*;

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

    let body_id_a = field_id(&repo, &tbl_a, "body").await;
    let body_id_b = field_id(&repo, &tbl_b, "body").await;

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
    let body_id = field_id(&repo, &tbl, "body").await;
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
async fn spawn_child_trunc(phase: &str, repo_path: &Path) -> (std::process::ExitStatus, String) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args([
        "--exact",
        "multi_table_drain::child_entrypoint_trunc",
        "--nocapture",
    ])
    .env(CHILD_SENTINEL_TRUNC, "1")
    .env(CRASH_AFTER, phase)
    .env(SEG_MAX_BYTES_ENV, TRUNC_SEG_CAP)
    .env(REPO_PATH, repo_path);
    super::child_proc::run_crash_child(phase, cmd).await
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

    let (status, child_output) = spawn_child_trunc(phase, &repo_path).await;
    assert!(
        !status.success(),
        "truncation child must die abnormally at seam {phase}; got a clean \
         exit (status {status:?}) — the crash seam did not fire (not enough \
         sealed segments to truncate?)\n{child_output}"
    );
    let sidecar = repo_path.with_extension("childcount");
    let durably_committed = read_sidecar_committed(&sidecar);

    let repo = open_repo(&repo_path).await;
    let _replayed = repo.recover_v2_inflight().await.expect("recovery");
    let tbl = repo.get_table(TABLE).await.unwrap();
    let data = store_record_count(&tbl).await;
    (data, durably_committed)
}

async fn spawn_child_multi_table(
    phase: &str,
    repo_path: &Path,
) -> (std::process::ExitStatus, String) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args([
        "--exact",
        "multi_table_drain::child_entrypoint_multi_table",
        "--nocapture",
    ])
    .env(CHILD_SENTINEL_MULTI, "1")
    .env(CRASH_AFTER, phase)
    .env(REPO_PATH, repo_path);
    super::child_proc::run_crash_child(phase, cmd).await
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

    let (status, child_output) = spawn_child_multi_table("phase4", &repo_path).await;
    assert!(
        !status.success(),
        "child must die abnormally on process::abort() at phase4; got {status:?}\n{child_output}"
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
