//! Online-copy gate test (brief 50-r2a-4, items 1–2): the real server
//! takes an ONLINE backup while four sequential writers keep committing
//! durable rows, then the snapshot is RESTORED onto an absent
//! clean-machine target and read back through the typed client.
//!
//! Certified invariants:
//! (a) Lower bound (design §6.1 / R2): every write acknowledged strictly
//!     BEFORE `StartBackup` was sent is present in the restored copy with
//!     its exact derived value, and per writer the restored sequences form
//!     the contiguous prefix `1..=m`, bounded by the writer row limit.
//! (b) Overlap + R3 writer-block bound (§6.2): at least one write per
//!     writer is acknowledged between the `StartBackup` request and the
//!     last observed live status; the tight Sealing window is
//!     MANDATORY (observed or the test fails) and every consecutive ack
//!     pair overlapping it stays within [`ACK_GAP_BOUND`]; the FIRST and
//!     LAST qualifying status observations carry strictly parsed
//!     `CopyProgress` amounts, both certified strictly below the final
//!     nonzero totals. `Verifying` and terminal bookkeeping are never
//!     treated as live (followup item 4: `active_seal_copy` decides).
//! (c) Non-vacuity: every writer ends in `WriterError::Stopped` with real
//!     progress — the stop signal, not the row limit, ends the run.
//! (d) Snapshot + restore (R4): the backup `dest_dir` exists and verifies
//!     (kind `online`, per-repo WAL position); `restore` with
//!     `force = false` onto an absent target succeeds with no unexplained
//!     `excluded_repositories`; restored scan and per-writer indexed
//!     lookups agree exactly and satisfy (a).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::query::Query;
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::read::query_record::QueryRecord;
use shamir_test_diag::FlakeGuard;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;
use tokio::task::JoinSet;

use super::copy_observation::{active_seal_copy, CopyProgress};
use super::fixture::{self, paced_execute, DB, REPO, REQUEST_TIMEOUT, TABLE};
use super::writers::{
    run_writer, value, Ack, Ledger, WriterControl, WriterError, WRITER_COUNT, WRITER_ROW_LIMIT,
};

/// Hard bound on consecutive ack-to-ack gaps per writer (R3, §6.2): 10×
/// the design's 2 s p99 per-ack baseline plus CI scheduling slack.
const ACK_GAP_BOUND: Duration = Duration::from_secs(30);

/// Minimum pacing between BackupStatus attempts (retries included); the
/// [`fixture::paced_execute`] send `Instant` is captured AFTER this sleep
/// and BEFORE the wire request.
pub(super) const STATUS_PACE: Duration = Duration::from_millis(20);

/// Total wall-clock budget for the whole test — the ONE overall scenario
/// bound: the FlakeGuard watchdog, the scenario timeout and every
/// in-scenario deadline derive from it (under the 180 s nextest per-test
/// kill). No stacked per-phase caps; no synchronization sleeps (bounded
/// requests + busy yields).
const TEST_BOUND: Duration = Duration::from_secs(170);

/// Stack for the deep synchronous restore/verify frames and for the
/// roomy thread that drives the whole scenario.
const BIG_STACK: usize = 64 * 1024 * 1024;

/// Stack for the runtime's worker threads.
const WORKER_STACK: usize = 16 * 1024 * 1024;

/// CI fixture padding: a 16 MiB inert, non-lock file in the source data
/// directory (`shamir_db_default_main.redb/zz-backup-test-fixture.bin`),
/// written AFTER setup, BEFORE writers, so the
/// online-backup job copies and hashes it through the real protocol in
/// both passes (file listing + hashing) and `verify_manifest` re-hashes
/// the restored copy. Its ONLY purpose is to widen the observable
/// post-capture interval (the backup does real 16 MiB of copying/hashing,
/// so live `BackupStatus` polls have more time to land) — no
/// synchronization sleeps, and the strict two-post-capture-response
/// non-vacuity requirement is NOT weakened. The size is a compile-time
/// constant, independent of writer retry/row counts.
const CI_FIXTURE_PAD_BYTES: usize = 16 * 1024 * 1024;
/// Chunk size for deterministic fixture writes (1 MiB per `write_all`).
const CI_FIXTURE_CHUNK_BYTES: usize = 1024 * 1024;
const CI_FIXTURE_PAD_FILE: &str = "zz-backup-test-fixture.bin";

/// Budget: total actual bytes under the data dir at quiet shutdown
/// (filesystem scan of every regular file, fixture padding included —
/// nothing is removed) must stay under 200 MB.
const DATA_DIR_TOTAL_BUDGET: u64 = 200 * 1024 * 1024;
/// Budget: bytes under retained WAL directories (every `*.shamirwal`
/// directory file) at quiet shutdown must stay under 32 MB.
const WAL_RETAINED_BUDGET: u64 = 32 * 1024 * 1024;

/// Deterministic NONZERO byte pattern for chunk `idx` of the fixture:
/// `(idx * 31 + 0x5A) as u8` — never zero, fully reproducible. Proof: the
/// addend 0x5A is nonzero, and `31 * idx mod 256` equals 0 only when
/// `idx` is a multiple of 256 (gcd(31, 256) = 1), where the pattern is
/// exactly 0x5A — so no `idx` yields 0.
fn fixture_pad_byte(chunk_idx: usize) -> u8 {
    (chunk_idx.wrapping_mul(31).wrapping_add(0x5A)) as u8
}

/// Write the inert fixture padding file directly into `data_dir` with
/// deterministic nonzero bytes, 1 MiB per chunk — an ordinary side file
/// in the `default/main` repo directory, no locks, no protocol.
fn ci_fixture_pad_path(base: &std::path::Path) -> std::path::PathBuf {
    base.join("shamir_db_default_main.redb")
        .join(CI_FIXTURE_PAD_FILE)
}

pub(super) fn write_ci_fixture_pad(data_dir: &std::path::Path) {
    let path = ci_fixture_pad_path(data_dir);
    let mut file = std::fs::File::create(&path).expect("create CI fixture pad file");
    // One reusable chunk buffer, refilled per pass — no 1 MiB clone per
    // chunk. `.fill` makes every byte of a chunk identical, so the bytes
    // written stay byte-for-byte identical to the old clone-per-chunk
    // form (each pass wrote a fresh vector of `fixture_pad_byte(idx)`).
    let mut buf = vec![0u8; CI_FIXTURE_CHUNK_BYTES];
    for idx in 0..(CI_FIXTURE_PAD_BYTES / CI_FIXTURE_CHUNK_BYTES) {
        buf.fill(fixture_pad_byte(idx));
        file.write_all(&buf)
            .expect("write CI fixture pad chunk (write_all: partial writes rejected)");
    }
    file.sync_all().expect("sync CI fixture pad file");
}

/// One recursive-scan outcome: total bytes of every regular file and the
/// subset under retained WAL directories (`*.shamirwal`).
#[derive(Debug, Default)]
struct DirScan {
    total_bytes: u64,
    wal_bytes: u64,
    fixture_bytes: u64,
}

/// Recursively sum regular-file sizes under `dir`. A file is WAL bytes
/// when ANY path component ends in `.shamirwal` (the per-repo WAL
/// directory shape: `<name>.shamirwal/` next to the repo dir) — this
/// counts ALL files inside those directories, not only `*.wal` segments.
/// Files are NEVER removed; symlinks are not followed (metadata only).
fn scan_dir(dir: &std::path::Path, under_wal: bool, scan: &mut DirScan, depth: u32) {
    assert!(
        depth < 32,
        "directory scan depth runaway at {}",
        dir.display()
    );
    for entry in dir
        .read_dir()
        .unwrap_or_else(|e| panic!("scan {}: {e}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("scan entry in {}: {e}", dir.display()));
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            panic!("scan metadata {}: vanished during scan", path.display())
        };
        if meta.is_dir() {
            let child_wal = under_wal
                || path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".shamirwal"));
            scan_dir(&path, child_wal, scan, depth + 1);
        } else if meta.is_file() {
            let size = meta.len();
            scan.total_bytes += size;
            if under_wal {
                scan.wal_bytes += size;
            }
            if path.file_name().is_some_and(|n| n == CI_FIXTURE_PAD_FILE) {
                scan.fixture_bytes += size;
            }
        }
    }
}

/// Assert the quiet-shutdown budgets over a full data-dir scan and verify
/// the fixture padding survived the round trip at its exact constant size.
fn assert_shutdown_budgets(data_dir: &std::path::Path, what: &str) {
    let mut scan = DirScan::default();
    scan_dir(data_dir, false, &mut scan, 0);
    assert_eq!(
        scan.fixture_bytes, CI_FIXTURE_PAD_BYTES as u64,
        "{what}: fixture padding must survive at exactly {CI_FIXTURE_PAD_BYTES} bytes (scan saw {})",
        scan.fixture_bytes
    );
    assert!(
        scan.total_bytes <= DATA_DIR_TOTAL_BUDGET,
        "{what}: data dir totals {} bytes, over the {DATA_DIR_TOTAL_BUDGET} budget",
        scan.total_bytes
    );
    assert!(
        scan.wal_bytes <= WAL_RETAINED_BUDGET,
        "{what}: retained WAL totals {} bytes, over the {WAL_RETAINED_BUDGET} budget",
        scan.wal_bytes
    );
}

/// Byte-verify the restored fixture padding against the deterministic
/// pattern (`fixture_pad_byte`), 1 MiB at a time — proves the REAL copy
/// + hash protocol carried the pad through both passes intact.
fn verify_restored_pad(restored_data_dir: &std::path::Path) {
    let path = ci_fixture_pad_path(restored_data_dir);
    let meta = std::fs::metadata(&path)
        .unwrap_or_else(|e| panic!("restored fixture pad {}: {e}", path.display()));
    assert_eq!(
        meta.len(),
        CI_FIXTURE_PAD_BYTES as u64,
        "restored fixture pad must be exactly {CI_FIXTURE_PAD_BYTES} bytes"
    );
    let mut file = std::fs::File::open(&path).expect("open restored fixture pad");
    let mut buf = vec![0u8; CI_FIXTURE_CHUNK_BYTES];
    for idx in 0..(CI_FIXTURE_PAD_BYTES / CI_FIXTURE_CHUNK_BYTES) {
        file.read_exact(&mut buf)
            .unwrap_or_else(|e| panic!("read restored pad chunk {idx}: {e}"));
        let expected = fixture_pad_byte(idx);
        assert!(
            buf.iter().all(|&b| b == expected),
            "restored pad chunk {idx} does not match the deterministic pattern"
        );
    }
}

/// One qualifying status observation of the live SEAL COPY: the wire
/// record showed THIS backup live in the `Sealing` stage (see
/// [`active_seal_copy`] — `Verifying` and terminal bookkeeping are NOT
/// live and never produce an observation). Status-placeholder note: while
/// the protocol runs the registry keeps `per_repo` EMPTY, which parses to
/// the zero progress, and an active seal with a malformed list yields
/// `None` from `active_seal_copy` — neither is treated as proof that no
/// copy happened; the timestamps are kept regardless of the amount.
struct SealObservation {
    /// `Instant` of the request send — taken BEFORE the request goes out.
    sent_at: Instant,
    /// `Instant` of the response receipt — set only AFTER the response
    /// arrived, never speculatively before send.
    received_at: Instant,
    /// Strictly parsed summed `per_repo` counters at poll time (may be
    /// the zero placeholder while the protocol runs).
    progress: CopyProgress,
}

/// Timeline edges of the observed copy window: the FIRST and the LAST
/// qualifying `Sealing` observations, each carrying both its request-send
/// and response-receipt `Instant`s and the parsed [`CopyProgress`].
struct StatusObservation {
    first: Option<SealObservation>,
    last: Option<SealObservation>,
}

/// Paced, bounded `execute` of a typed batch (pacing/retry via
/// [`fixture::paced_execute`]: ≥ [`STATUS_PACE`] before the send, exact
/// pre-dispatch `rate_limited` retried within the shared 120 s budget);
/// panics on transport failure or overrun (certification failure).
async fn bounded_execute(
    client: &shamir_client::Client,
    db: &str,
    batch: shamir_query_types::batch::BatchRequest,
    what: &'static str,
) -> BatchResponse {
    let (_, result) = paced_execute(client, db, batch, STATUS_PACE).await;
    result.unwrap_or_else(|e| panic!("{what}: execute failed: {e}"))
}

/// The single direct record of a single-alias response.
///
/// # Panics
/// When the alias is missing, cascade-skipped, or returned no record.
fn single_record(mut response: BatchResponse, alias: &str, what: &'static str) -> QueryRecord {
    let result = response
        .results
        .shift_remove(alias)
        .unwrap_or_else(|| panic!("{what}: response missing the '{alias}' alias"));
    assert!(!result.skipped, "{what}: '{alias}' was cascade-skipped");
    result
        .records
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{what}: '{alias}' returned no records"))
}

/// The row field values of one record: `(writer, seq, value)`.
///
/// # Panics
/// When any field is missing or not an integer (a torn record is a
/// certification failure, never tolerated).
fn row_triple(record: &QueryRecord, what: &'static str) -> (i64, i64, i64) {
    let writer = record
        .get_value_i64("writer")
        .unwrap_or_else(|| panic!("{what}: row missing integer 'writer': {record:?}"));
    let seq = record
        .get_value_i64("seq")
        .unwrap_or_else(|| panic!("{what}: row missing integer 'seq': {record:?}"));
    let stored = record
        .get_value_i64("value")
        .unwrap_or_else(|| panic!("{what}: row missing integer 'value': {record:?}"));
    (writer, seq, stored)
}

/// One `BackupStatus` poll: a PACED bounded request via
/// [`fixture::paced_execute`]; returns the send `Instant` (after the
/// pacing sleep, before the SUCCESSFUL wire request, post-retry) and the
/// record.
async fn poll_backup_status(client: &shamir_client::Client) -> (Instant, QueryRecord) {
    let mut batch = Batch::new();
    batch.id("backup-status");
    batch.op("backup-status", ddl::backup_status());
    let (sent_at, result) = paced_execute(client, "default", batch.build(), STATUS_PACE).await;
    let response = result.unwrap_or_else(|e| panic!("backup status: execute failed: {e}"));
    (
        sent_at,
        single_record(response, "backup-status", "backup status"),
    )
}

/// Poll `BackupStatus` until `running == false`; returns the final status
/// record and the overlap observations. Fails on `Failed` stage, any
/// present `last_error`, or the phase deadline. No synchronization sleeps.
async fn await_backup_done(
    client: &shamir_client::Client,
    backup_id: &str,
    guard: &FlakeGuard,
    deadline: Instant,
) -> (QueryRecord, StatusObservation) {
    let mut observation = StatusObservation {
        first: None,
        last: None,
    };
    let mut last_stage = String::from("(none)");
    loop {
        assert!(
            Instant::now() < deadline,
            "backup did not finish within the phase deadline; last stage: {last_stage}"
        );
        // The LAST request `Instant` is stamped by `paced_execute` after
        // the pacing sleep and before the SUCCESSFUL wire request
        // (post-retry); the response `Instant` is set after receipt.
        let (sent_at, status) = poll_backup_status(client).await;
        let received_at = Instant::now();
        let stage = status
            .get_value_str("stage")
            .unwrap_or_else(|| panic!("backup status: record missing 'stage'"))
            .to_owned();
        assert!(
            stage != "Failed",
            "online backup reported stage Failed; status record: {status:?}"
        );
        assert!(
            status.get_value("last_error").is_none(),
            "online backup reported last_error: {:?}",
            status.get_value_str("last_error")
        );
        // ONLY an active `Sealing` seal of THIS backup counts as a live
        // observation: `Verifying` / final bookkeeping / other runs are
        // never treated as live and contribute no timestamps. A `None`
        // from `active_seal_copy` during a live seal (placeholder or
        // malformed `per_repo`) is not proof of no copy — no observation
        // is recorded for it, and the final-totals assertions below
        // certify from the observations that DID qualify.
        if let Some(progress) = active_seal_copy(&status, backup_id) {
            if observation.first.is_none() {
                guard.mark("seal-copy window opens (first Sealing response received)");
                observation.first = Some(SealObservation {
                    sent_at,
                    received_at,
                    progress,
                });
            }
            observation.last = Some(SealObservation {
                sent_at,
                received_at,
                progress,
            });
        }
        let running = status
            .get_value("running")
            .and_then(QueryValue::as_bool)
            .unwrap_or_else(|| panic!("backup status: 'running' must be a bool"));
        if !running {
            assert_eq!(
                stage, "Done",
                "finished backup must report Done (got '{stage}')"
            );
            guard.mark("backup Done");
            return (status, observation);
        }
        last_stage = stage;
        tokio::task::yield_now().await;
    }
}

/// Per-writer contiguity + derivation + R3 gap check over the ack ledger.
fn certify_acks(snapshot: &[Ack]) {
    let mut per_writer: Vec<Vec<&Ack>> = vec![Vec::new(); WRITER_COUNT + 1];
    for ack in snapshot {
        per_writer[ack.writer as usize].push(ack);
    }
    for (writer, acks) in per_writer.iter().enumerate().skip(1) {
        assert!(
            !acks.is_empty(),
            "writer {writer} produced no acknowledged writes"
        );
        for (expected, ack) in acks.iter().enumerate() {
            let seq = expected as u64 + 1;
            assert_eq!(
                ack.seq, seq,
                "writer {writer} acks must be contiguous from 1"
            );
            assert_eq!(
                ack.value,
                value(writer as u64, seq),
                "writer {writer} ack value must match value(writer, seq)"
            );
            if expected > 0 {
                let gap = ack.acked_at.duration_since(acks[expected - 1].acked_at);
                assert!(
                    gap <= ACK_GAP_BOUND,
                    "writer {writer} stalled {gap:?} between acks {} and {} (bound {ACK_GAP_BOUND:?})",
                    seq - 1,
                    seq,
                );
            }
        }
    }
}

/// Fold every returned row (result.records are `Vec<QueryRecord>`) into
/// `restored`, refusing duplicates and rejecting any row outside the
/// certified domain: writers `1..=WRITER_COUNT`, seqs `1..=WRITER_ROW_LIMIT`
/// (an extra writer or out-of-range prefix is a torn/corrupt restore).
fn fold_rows(
    restored: &mut BTreeMap<(i64, i64), i64>,
    response: BatchResponse,
    alias: &str,
    what: &'static str,
) {
    let result = response
        .results
        .get(alias)
        .unwrap_or_else(|| panic!("{what}: response missing the '{alias}' alias"));
    assert!(!result.skipped, "{what}: '{alias}' was cascade-skipped");
    for record in &result.records {
        let (writer, seq, stored) = row_triple(record, what);
        assert!(
            (1..=WRITER_COUNT as i64).contains(&writer),
            "{what}: row for out-of-domain writer {writer}"
        );
        assert!(
            (1..=WRITER_ROW_LIMIT as i64).contains(&seq),
            "{what}: row for out-of-range seq {seq} (writer {writer})"
        );
        let previous = restored.insert((writer, seq), stored);
        assert!(
            previous.is_none(),
            "{what}: duplicate row for writer {writer} seq {seq} (previous {previous:?}, new {stored})"
        );
    }
}

/// Read the restored `rows` table: the unfiltered full scan and the
/// indexed per-writer lookup for every writer; the per-writer result must
/// EQUAL the scan's slice for that writer.
async fn read_restored_rows(client: &shamir_client::Client) -> BTreeMap<(i64, i64), i64> {
    let mut restored: BTreeMap<(i64, i64), i64> = BTreeMap::new();

    let mut scan = Batch::new();
    scan.id("restored-scan");
    scan.query(
        "scan",
        Query::with_repo(REPO, TABLE)
            .select(["writer", "seq", "value"])
            .where_gte("seq", 1_i64)
            .build(),
    );
    let response = bounded_execute(client, DB, scan.build(), "restored full scan").await;
    let baseline = response
        .results
        .get("scan")
        .unwrap_or_else(|| panic!("restored full scan: missing 'scan' alias"));
    let baseline_stats = baseline
        .stats
        .as_ref()
        .expect("restored full scan: missing query stats");
    assert_eq!(
        baseline_stats.index_used.as_deref(),
        None,
        "baseline scan must not use an index (only 'writer' is indexed)"
    );
    fold_rows(&mut restored, response, "scan", "restored full scan");

    for writer in 1..=WRITER_COUNT as i64 {
        let mut batch = Batch::new();
        batch.id("restored-by-writer");
        batch.query(
            "by-writer",
            Query::with_repo(REPO, TABLE)
                .select(["writer", "seq", "value"])
                .where_eq("writer", writer)
                .build(),
        );
        let response =
            bounded_execute(client, DB, batch.build(), "restored per-writer lookup").await;
        let result = response
            .results
            .get("by-writer")
            .unwrap_or_else(|| panic!("per-writer lookup: missing 'by-writer' alias"));
        assert!(!result.skipped, "per-writer lookup was cascade-skipped");
        assert_eq!(
            result
                .stats
                .as_ref()
                .expect("per-writer lookup: missing query stats")
                .index_used
                .as_deref(),
            Some("rows_hash_idx"),
            "per-writer lookup for writer {writer} must use rows_hash_idx"
        );
        let mut per_writer: BTreeMap<i64, i64> = BTreeMap::new();
        for record in &result.records {
            let (row_writer, seq, stored) = row_triple(record, "restored per-writer lookup");
            assert_eq!(
                row_writer, writer,
                "indexed lookup for writer {writer} returned a row of writer {row_writer}"
            );
            let previous = per_writer.insert(seq, stored);
            assert!(
                previous.is_none(),
                "per-writer lookup: duplicate seq {seq} for writer {writer}"
            );
        }
        let scanned: BTreeMap<i64, i64> = restored
            .iter()
            .filter(|((w, _), _)| *w == writer)
            .map(|((_, s), v)| (*s, *v))
            .collect();
        assert_eq!(
            &scanned, &per_writer,
            "writer {writer}: indexed lookup must return exactly the rows the full scan returns"
        );
    }
    restored
}

/// Run the synchronous `restore(from → data_dir, force = false)` on a
/// dedicated roomy thread, bounded by `budget` (remaining scenario
/// deadline capped at [`REQUEST_TIMEOUT`], so a started restore never
/// pushes the total past the outer cap).
fn bounded_restore(
    guard: &FlakeGuard,
    from: std::path::PathBuf,
    data_dir: std::path::PathBuf,
    budget: Duration,
) -> shamir_server::restore::RestoreReport {
    guard.mark("restore thread starts");
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("gate-restore".into())
        .stack_size(BIG_STACK)
        .spawn(move || {
            // force = false: the clean-machine path — `data_dir` does not
            // exist yet, so the liveness probe passes trivially.
            let _ = tx.send(shamir_server::restore::restore(&from, &data_dir, false));
        })
        .expect("spawn restore thread");
    let outcome = rx
        .recv_timeout(budget)
        .unwrap_or_else(|_| panic!("restore did not complete within {budget:?}"));
    guard.mark("restore thread finished");
    let report = outcome.unwrap_or_else(|e| panic!("restore failed: {e}"));
    assert!(
        report.excluded_repositories.is_empty(),
        "restore excluded repositories without an explained cause: {:?}",
        report.excluded_repositories
    );
    assert!(
        report.files_restored > 0 && report.bytes_restored > 0,
        "restore must have restored real content: {} files / {} bytes",
        report.files_restored,
        report.bytes_restored
    );
    report
}

/// The gate scenario, run inside the test runtime.
async fn gate_scenario(guard: FlakeGuard) {
    let started_at = Instant::now();
    let overall_deadline = started_at + TEST_BOUND;

    let data_dir = TempDir::new().expect("temp data dir");
    let backup_root = TempDir::new().expect("temp backup root");
    let restore_root = TempDir::new().expect("temp restore root");
    let restored_data_dir = restore_root.path().join("data");
    assert!(!restored_data_dir.exists(), "restore target must be absent");

    let booted = fixture::boot(data_dir.path(), true).await;
    let addr = booted.addr();
    let admin = fixture::connect(addr, &booted.password, "admin").await;
    fixture::setup(&admin).await;
    guard.mark("server booted, fixture set up");

    // CI fixture padding (written AFTER setup, BEFORE writers): inert side
    // file inside the real `default/main` repo directory the online-backup
    // job copies and hashes through the real protocol in both passes,
    // widening the observable post-capture interval without sleeps or
    // injected pauses.
    write_ci_fixture_pad(data_dir.path());
    guard.mark("CI fixture pad written (16 MiB)");

    // ── writers: four sequential loops on their own connections ──
    let control = WriterControl::new();
    let ledger = Ledger::new();
    let mut writers: JoinSet<Result<(), WriterError>> = JoinSet::new();
    for writer in 1..=WRITER_COUNT {
        let client =
            fixture::connect(addr, &booted.password, format!("writer {writer}").as_str()).await;
        let rx = control.subscribe();
        let task_ledger = ledger.clone();
        writers.spawn(async move { run_writer(writer as u64, client, rx, &task_ledger).await });
    }

    // (a-warmup) Every writer has ≥1 ack BEFORE StartBackup (one overall
    // bound — the fixture's 120 s deadline).
    let warmup_deadline = overall_deadline.min(Instant::now() + REQUEST_TIMEOUT);
    loop {
        let snapshot = ledger.snapshot();
        let all_warmed = (1..=WRITER_COUNT as u64)
            .all(|w| snapshot.iter().filter(|ack| ack.writer == w).count() >= 1);
        if all_warmed {
            break;
        }
        assert!(
            Instant::now() < warmup_deadline,
            "writers did not each acknowledge one write in time"
        );
        tokio::task::yield_now().await;
    }
    guard.mark("all writers warmed up (≥1 ack each)");

    // ── StartBackup: the send `Instant` is the paced_execute SEND stamp
    // (after pacing, before the successful wire request); the ledger
    // stamps REAL ack receipts, so `acked_at < start_sent_at` proves
    // acknowledgment before StartBackup went out (the server-side
    // capture barrier sits after this request). ──
    let mut start = Batch::new();
    start.id("start-backup");
    start.op(
        "start",
        ddl::start_backup(backup_root.path().display().to_string()),
    );
    let (start_sent_at, start_result) =
        paced_execute(&admin, "default", start.build(), STATUS_PACE).await;
    let response = start_result.unwrap_or_else(|e| panic!("start backup: execute failed: {e}"));
    let started_record = single_record(response, "start", "start backup");
    let backup_id = started_record
        .get_value_str("backup_id")
        .unwrap_or_else(|| panic!("start backup: record missing 'backup_id'"))
        .to_owned();
    assert!(
        !backup_id.is_empty(),
        "start backup: 'backup_id' must be non-empty"
    );
    guard.note("backup_id", &backup_id);

    // (b) Backup runs to Done while writers keep committing (one overall
    // bound — `TEST_BOUND`).
    let backup_deadline = overall_deadline.min(Instant::now() + REQUEST_TIMEOUT);
    let (final_status, observation) =
        await_backup_done(&admin, &backup_id, &guard, backup_deadline).await;

    // Final nonzero copy totals, parsed from the Done record (the registry
    // fills `per_repo` only after the protocol returns, so the terminal
    // record carries the REAL amounts, not a placeholder).
    let final_progress = CopyProgress::from_status(&final_status)
        .expect("final Done status must parse progress.per_repo");

    // The final `dest_dir` is ONLY reported in the BackupStatus response.
    let dest_dir_str = final_status
        .get_value_str("dest_dir")
        .unwrap_or_else(|| panic!("final status: record missing 'dest_dir'"))
        .to_owned();
    let dest_dir = std::path::PathBuf::from(&dest_dir_str);
    assert_eq!(
        final_status
            .get_value_str("backup_id")
            .unwrap_or_else(|| panic!("final status: record missing 'backup_id'")),
        backup_id,
        "final BackupStatus must report the started backup's id"
    );
    assert!(
        final_status.get_value("last_error").is_none(),
        "successful backup status must omit last_error"
    );

    // ── stop the fleet, THEN join ──
    control.stop();
    let mut outcomes = Vec::with_capacity(WRITER_COUNT);
    while let Some(outcome) = writers.join_next().await {
        outcomes.push(outcome.expect("writer task must not panic"));
    }
    drop(control);

    // (c) Non-vacuity: every writer ended Stopped, with live progress.
    assert_eq!(
        outcomes.len(),
        WRITER_COUNT,
        "all four writers must be joined"
    );
    for (idx, outcome) in outcomes.iter().enumerate() {
        match outcome {
            Ok(()) => panic!(
                "writer {} returned Ok; the stop signal must end it",
                idx + 1
            ),
            Err(WriterError::Stopped { next_seq }) => assert!(
                *next_seq >= 2,
                "writer {} stopped without real progress (next_seq {next_seq})",
                idx + 1
            ),
            Err(WriterError::Exhausted { limit }) => panic!(
                "writer {} exhausted its limit {limit}; the backup window was too slow \
                 (writers must be stopped, not run dry)",
                idx + 1
            ),
            Err(WriterError::Request { seq, source }) => {
                panic!("writer {} request failed at seq {seq}: {source}", idx + 1)
            }
        }
    }

    let snapshot = ledger.snapshot();
    certify_acks(&snapshot);

    // (a-lower) The full pre-send set comes from the FINAL ledger, whose
    // `acked_at` stamps are real ack receipts filtered strictly before
    // `start_sent_at` (an early snapshot would freeze before the request
    // was even built).
    let pre_start: Vec<&Ack> = snapshot
        .iter()
        .filter(|ack| ack.acked_at < start_sent_at)
        .collect();
    let pre_start_counts: Vec<usize> = (0..=WRITER_COUNT)
        .map(|w| {
            pre_start
                .iter()
                .filter(|ack| ack.writer == w as u64)
                .count()
        })
        .collect();
    // (a-upper) Writers all stop cleanly, hence every write attempt was
    // acknowledged: the final ack count per writer bounds the restored
    // prefix from above.
    let final_counts: Vec<u64> = (0..=WRITER_COUNT)
        .map(|w| snapshot.iter().filter(|ack| ack.writer == w as u64).count() as u64)
        .collect();

    // (b-overlap) MANDATORY: at least one ack per writer between the
    // StartBackup request and the LAST live status — real overlap for
    // EVERY writer (acks are receipt stamps).
    let last_live = observation
        .last
        .expect("no live Sealing status was ever observed — overlap cannot be certified");
    let first_live = observation
        .first
        .expect("live statuses observed without a first observation");
    // Endpoint instants: window OPENS at the first response receipt,
    // CLOSES at the last request send.
    let last_live_at = last_live.sent_at;
    let overlap_lower = first_live.received_at;
    assert!(
        overlap_lower <= last_live_at,
        "overlap window ill-formed (lower {overlap_lower:?} > upper {last_live_at:?})"
    );
    // Both observed amounts must sit strictly below the final nonzero
    // totals on BOTH counters — the polls landed mid-copy, not after.
    first_live.progress.assert_before_final(final_progress);
    last_live.progress.assert_before_final(final_progress);
    for writer in 1..=WRITER_COUNT {
        assert!(
            snapshot.iter().any(|ack| {
                ack.writer == writer as u64
                    && ack.acked_at >= start_sent_at
                    && ack.acked_at < last_live_at
            }),
            "writer {writer} had no acknowledgement between StartBackup and the last live status"
        );
    }
    // STRICT window: lower is post-receipt, upper pre-send — boundary
    // acks are excluded and the comparisons are strict.
    let in_window: Vec<&Ack> = snapshot
        .iter()
        .filter(|ack| ack.acked_at > overlap_lower && ack.acked_at < last_live_at)
        .collect();
    guard.note("acks in tight window", in_window.len().to_string());
    assert!(
        !in_window.is_empty(),
        "no acks landed inside the tight live window"
    );
    // Consecutive-ack gaps PER WRITER: pairing across writers would pair
    // unrelated acks and measure nothing. In-window order is global ack
    // order, so each writer's subsequence stays time-sorted.
    for writer in 1..=WRITER_COUNT {
        let mut previous: Option<&Ack> = None;
        for ack in in_window.iter().filter(|ack| ack.writer == writer as u64) {
            if let Some(prev) = previous {
                let gap = ack.acked_at.duration_since(prev.acked_at);
                assert!(
                    gap < ACK_GAP_BOUND,
                    "writer {writer} stalled {gap:?} between acks inside the overlap window (bound {ACK_GAP_BOUND:?})"
                );
            }
            previous = Some(ack);
        }
    }

    // (d) Graceful source shutdown BEFORE the snapshot is restored
    // (fjall file locks on Windows).
    fixture::shutdown(admin, booted).await;
    guard.mark("source server shut down cleanly");
    assert!(
        Instant::now() < overall_deadline,
        "scenario exceeded the total {TEST_BOUND:?} budget before restore"
    );

    // (d-snapshot) dest_dir exists, verifies, kind online, per-repo WAL pos.
    assert!(
        dest_dir.is_dir(),
        "backup dest_dir {dest_dir_str:?} must exist after Done"
    );
    assert!(
        dest_dir
            .read_dir()
            .expect("read backup dir")
            .next()
            .is_some(),
        "backup dest_dir {dest_dir_str:?} must not be empty"
    );
    let verify = shamir_server::backup::verify_manifest(&dest_dir)
        .unwrap_or_else(|e| panic!("snapshot manifest must verify: {e}"));
    assert!(
        matches!(
            verify.kind,
            shamir_server::backup::online::manifest::BackupKind::Online
        ),
        "manifest kind must be online"
    );
    let consistency = verify
        .consistency
        .as_ref()
        .expect("online manifest must carry a consistency section");
    let repo_key = format!("{DB}/{REPO}");
    let repo_point = consistency
        .per_repo
        .get(&repo_key)
        .unwrap_or_else(|| panic!("manifest consistency must carry a position for '{repo_key}'"));
    assert!(
        repo_point.wal_position >= 1,
        "manifest WAL position for '{repo_key}' must cover the pre-barrier writes (got {})",
        repo_point.wal_position
    );
    guard.note(
        format!("wal_position[{repo_key}]"),
        repo_point.wal_position.to_string(),
    );

    // (d-restore) Clean-machine restore onto the ABSENT target; budget
    // min(120 s, remaining deadline) stays under the outer cap.
    let restore_budget =
        REQUEST_TIMEOUT.min(overall_deadline.saturating_duration_since(Instant::now()));
    let report = bounded_restore(&guard, dest_dir, restored_data_dir.clone(), restore_budget);
    guard.note(
        "restored files/bytes",
        format!("{} / {}", report.files_restored, report.bytes_restored),
    );

    let restored_boot = fixture::boot(&restored_data_dir, false).await;
    let restored_client =
        fixture::connect(restored_boot.addr(), &restored_boot.password, "restored").await;
    let restored = read_restored_rows(&restored_client).await;
    assert!(!restored.is_empty(), "the restored copy must not be empty");

    // The real copy+hash protocol carried the fixture pad through both
    // passes: byte-verify the restored padding against the deterministic
    // pattern.
    verify_restored_pad(&restored_data_dir);

    // (a) Exact lower bound: every pre-start ack present with its value.
    for ack in &pre_start {
        let stored = restored
            .get(&(ack.writer as i64, ack.seq as i64))
            .unwrap_or_else(|| {
                panic!(
                    "restored copy lost ack (writer {} seq {})",
                    ack.writer, ack.seq
                )
            });
        assert_eq!(
            *stored, ack.value as i64,
            "restored value for (writer {} seq {}) must match the derivation",
            ack.writer, ack.seq
        );
    }

    // (a-prefix) Per writer the restored sequences are the contiguous
    // prefix 1..=m, pre-start count ≤ m ≤ final ack count, values match
    // the derivation — no holes/extras.
    for writer in 1..=WRITER_COUNT as u64 {
        let seqs: Vec<i64> = restored
            .keys()
            .filter(|(w, _)| *w == writer as i64)
            .map(|(_, s)| *s)
            .collect();
        let m = seqs.len();
        assert!(
            m >= pre_start_counts[writer as usize],
            "writer {writer}: restored prefix length {m} below the certified lower bound {}",
            pre_start_counts[writer as usize]
        );
        assert!(
            m as u64 <= final_counts[writer as usize],
            "writer {writer}: restored prefix length {m} exceeds the final ack count {}",
            final_counts[writer as usize]
        );
        for (offset, seq) in seqs.iter().enumerate() {
            assert_eq!(
                *seq,
                offset as i64 + 1,
                "writer {writer}: restored sequences must be the contiguous prefix 1..={m}"
            );
            let stored = restored
                .get(&(writer as i64, *seq))
                .unwrap_or_else(|| panic!("writer {writer} seq {seq} missing"));
            assert_eq!(
                *stored,
                value(writer, *seq as u64) as i64,
                "writer {writer} seq {seq}: restored value must match the derivation"
            );
        }
    }

    // ── clean teardown of the restored server ──
    fixture::shutdown(restored_client, restored_boot).await;
    guard.mark("restored server shut down cleanly");

    // ── quiet-shutdown budget scan (nothing removed, fixture included) ──
    // Both data dirs are quiescent (each server shut down cleanly); the
    // scan counts every regular file and removes nothing.
    assert_shutdown_budgets(data_dir.path(), "source data dir");
    assert_shutdown_budgets(&restored_data_dir, "restored data dir");
    guard.mark("shutdown budget scan passed");
    assert!(
        Instant::now() < overall_deadline,
        "scenario exceeded the total {TEST_BOUND:?} budget"
    );
}

/// The gate test: the async scenario runs on a roomy 64 MiB thread (the
/// synchronous restore/verify frames are deep) with a 16 MiB-per-worker
/// runtime, under both an in-scenario timeout and the [`TEST_BOUND`]
/// watchdog — all under the 180 s nextest per-test kill.
#[test]
fn online_backup_overlaps_writers_restores_clean_machine_and_keeps_every_ack() {
    let guard = FlakeGuard::with_watchdog(
        "online_backup gate: overlap + clean-machine restore",
        TEST_BOUND,
    );
    guard.mark("gate test starts");
    let (tx, rx) = mpsc::channel::<Result<(), tokio::time::error::Elapsed>>();
    let handle = std::thread::Builder::new()
        .name("gate-main".into())
        .stack_size(BIG_STACK)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_stack_size(WORKER_STACK)
                .enable_all()
                .build()
                .expect("build the gate test runtime");
            let outcome = runtime
                .block_on(async { tokio::time::timeout(TEST_BOUND, gate_scenario(guard)).await });
            let _ = tx.send(outcome);
        })
        .expect("spawn the roomy gate thread");
    match rx.recv_timeout(TEST_BOUND + Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(_elapsed)) => panic!("gate scenario exceeded the overall {TEST_BOUND:?} timeout"),
        Err(_) => panic!(
            "gate thread panicked or did not finish within its bound ({} s)",
            (TEST_BOUND + Duration::from_secs(5)).as_secs()
        ),
    }
    // Not detached on the success path: a panic on the gate thread after
    // the result was sent still fails the test (JoinHandle::join
    // propagates it).
    handle
        .join()
        .expect("gate thread must not panic after reporting its outcome");
}
