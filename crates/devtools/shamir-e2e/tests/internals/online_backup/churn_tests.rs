//! Catalogue-churn variant of the online-copy gate (brief 50-r2a-4, item 1
//! variant; design Q9): a SECOND PERSISTENT repository, its table and one
//! durable write are created THROUGH THE REAL CLIENT while the online copy
//! is live, and the restore still lands clean.
//!
//! Certified invariants (each stated with its proof source):
//! (a) Overlap is REAL, not assumed: the churn DDL/write sequence is
//!     issued only after a `BackupStatus` poll observed an ACTIVE SEAL COPY
//!     (`running == true`, stage `Sealing`, the STARTED backup's id — via
//!     `super::copy_observation::active_seal_copy`, so a `Verifying` or
//!     bookkeeping poll can never count), and at least one further ACTIVE
//!     SEAL COPY is observed by a status request sent strictly AFTER the
//!     churn write was acknowledged — the copy was still mid-seal when the
//!     churn write committed.
//! (b) The captured set is FIXED at capture: the snapshot manifest's
//!     `consistency.per_repo` is EXACTLY `{"default/main",
//!     "backup_load/main"}` — the bootstrap-created persistent
//!     `default/main` plus the fixture's captured repo; the mid-copy churn
//!     repository never enters it.
//! (c) The restore is CLEAN and every exclusion is NAMED and explained:
//!     the report's `excluded_repositories` contains only
//!     `backup_load/churn` (Q9's created-after-capture repo) — never an
//!     unexplained entry; the churn repository is never registered by the
//!     restored boot, and the captured `main` repository is intact.
//!
//! Source obstruction, reported (not worked around with a flaky assert):
//! WHETHER `backup_load/churn` appears in `excluded_repositories` depends
//! on a race the real client cannot observe or order — the churn commit
//! versus the protocol's seal RESIDUAL pass copying the SYSTEM-store
//! catalogue/WAL into the snapshot (protocol.rs: the residual pass runs
//! after the per-repo seal re-copies with no port hint, and the adapter
//! exposes no stage signal for it). If the commit lands before the
//! residual copy, the staged catalogue carries the churn row and restore
//! names it in `excluded_repositories` (removing it from the staged
//! catalogue, design Q8); if it lands after, the snapshot catalogue simply
//! predates the churn repo and the exclusion set is empty. Both outcomes
//! are legal; this test accepts exactly those two and rejects anything
//! else. The deterministic created-after-capture exclusion oracle already
//! exists at the server level
//! (`db_handler/tests/policy/backup_online/restore/
//! repo_created_after_capture_is_excluded_and_not_registered_on_boot`).

use std::sync::mpsc;
use std::time::{Duration, Instant};

use shamir_client::builder::batch::{Batch, Durability};
use shamir_client::builder::ddl::{self, RepoEngine};
use shamir_client::builder::query::Query;
use shamir_client::builder::write;
use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_query_types::read::query_record::QueryRecord;
use shamir_test_diag::FlakeGuard;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;

use super::copy_observation::{active_seal_copy, CopyProgress};
use super::fixture::{self, DB, REPO, REQUEST_TIMEOUT, TABLE};

/// The second, mid-copy repository (Q9's created-after-capture repo).
const CHURN_REPO: &str = "churn";
/// The churn repository's table.
const CHURN_TABLE: &str = "churn_rows";

/// Churn row payload columns; the value is derived so a torn record is
/// detectable.
const CHURN_WRITER: u64 = 1;
const CHURN_SEQ: u64 = 1;
const CHURN_VALUE: u64 = 42;

/// Total wall-clock budget for the whole test, under the 180 s nextest
/// per-test kill; the FlakeGuard watchdog and the scenario timeout.
const TEST_BOUND: Duration = Duration::from_secs(170);

/// Per-phase wall-clock budget inside the scenario.
const TOTAL_BOUND: Duration = Duration::from_secs(150);

/// Bound for the synchronous `restore` on its dedicated big-stack thread.
const RESTORE_BOUND: Duration = Duration::from_secs(90);

/// Stack for the deep synchronous restore/verify frames and the roomy
/// scenario thread.
const BIG_STACK: usize = 64 * 1024 * 1024;

/// Stack for the runtime's worker threads.
const WORKER_STACK: usize = 16 * 1024 * 1024;

/// A `BackupStatus` record's stage name (`Debug` of `BackupStage`).
fn stage_of(status: &QueryRecord) -> String {
    status
        .get_value_str("stage")
        .unwrap_or_else(|| panic!("backup status: record missing 'stage'"))
        .to_owned()
}

/// The `running` bool of a status record.
fn running_of(status: &QueryRecord) -> bool {
    status
        .get_value("running")
        .and_then(QueryValue::as_bool)
        .unwrap_or_else(|| panic!("backup status: 'running' must be a bool"))
}

/// Bounded `execute` of a typed batch; panics on transport failure or
/// overrun (a certification failure, never a soft skip).
async fn bounded_execute(
    client: &shamir_client::Client,
    db: &str,
    batch: BatchRequest,
    what: &'static str,
) -> BatchResponse {
    match tokio::time::timeout(REQUEST_TIMEOUT, client.execute(db, batch)).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("{what}: execute failed: {e}"),
        Err(_elapsed) => panic!("{what}: exceeded {REQUEST_TIMEOUT:?}"),
    }
}

/// One `BackupStatus` poll (bounded request) as a raw record.
async fn poll_backup_status(client: &shamir_client::Client) -> QueryRecord {
    let mut batch = Batch::new();
    batch.id("churn-backup-status");
    batch.op("backup-status", ddl::backup_status());
    let mut response =
        bounded_execute(client, "default", batch.build(), "churn backup status").await;
    let result = response
        .results
        .shift_remove("backup-status")
        .unwrap_or_else(|| panic!("churn backup status: missing 'backup-status' alias"));
    assert!(
        !result.skipped,
        "churn backup status: 'backup-status' was cascade-skipped"
    );
    result
        .records
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("churn backup status: no records"))
}

/// One bounded, verified typed batch (single alias; DDL batches carry no
/// transaction on the wire, so no transaction is asserted here — the one
/// insert of the scenario checks its transaction inline).
async fn run_step(client: &shamir_client::Client, what: &'static str, batch: Batch) {
    let mut response = bounded_execute(client, DB, batch.build(), what).await;
    let result = response
        .results
        .shift_remove(what)
        .unwrap_or_else(|| panic!("{what}: missing alias"));
    assert!(!result.skipped, "{what}: cascade-skipped");
}

/// The churn scenario, run inside the test runtime.
async fn churn_scenario(guard: FlakeGuard) {
    let overall_deadline = Instant::now() + TOTAL_BOUND;

    let data_dir = TempDir::new().expect("temp data dir");
    let backup_root = TempDir::new().expect("temp backup root");
    let restore_root = TempDir::new().expect("temp restore root");
    let restored_data_dir = restore_root.path().join("data");
    assert!(!restored_data_dir.exists(), "restore target must be absent");

    let booted = fixture::boot(data_dir.path(), true).await;
    let admin = fixture::connect(booted.addr(), &booted.password).await;
    fixture::setup(&admin).await;
    guard.mark("server booted, fixture set up (no churn repo yet)");

    // CI fixture padding (written AFTER setup, BEFORE writers): the same
    // inert 16 MiB side file the gate test uses. Copying and hashing it
    // through the real protocol in both passes widens the observable
    // post-capture interval without sleeps or injected pauses — the one
    // synced seed row alone is not enough workload for the tight-window
    // overlap to be observable.
    super::gate_tests::write_ci_fixture_pad(data_dir.path());
    guard.mark("CI fixture pad written (16 MiB)");

    // One seeded, SYNCED row in `backup_load/main` BEFORE the backup: the
    // inert workload of this variant. It makes the manifest's WAL position
    // and the restored-table assertion non-vacuous (an empty table would
    // also "restore" from an empty capture) while staying far below the
    // 200 MB directory budget.
    {
        let doc = write::doc()
            .set_value("writer", 1_u64)
            .set_value("seq", 1_u64)
            .set_value("value", 7_u64)
            .build();
        let mut batch = Batch::new();
        batch.id("seed-main-row");
        batch.transactional();
        batch.durability(Durability::Synced);
        batch.insert(TABLE, write::Insert::with_repo(REPO, TABLE).row(doc));
        let mut response = bounded_execute(&admin, DB, batch.build(), "seed-main-row").await;
        let result = response
            .results
            .shift_remove(TABLE)
            .unwrap_or_else(|| panic!("seed-main-row: missing '{TABLE}' alias"));
        assert!(!result.skipped, "seed-main-row: cascade-skipped");
        match response.transaction {
            Some(tx) if tx.is_committed() && tx.commit_version.is_some() => {}
            other => panic!("seed-main-row: not committed with a commit_version: {other:?}"),
        }
    }
    guard.mark("seeded one synced row in backup_load/main");

    // ── StartBackup (typed admin op, bounded) ──
    let mut start = Batch::new();
    start.id("churn-start-backup");
    start.op(
        "churn-start",
        ddl::start_backup(backup_root.path().display().to_string()),
    );
    let mut start_response =
        bounded_execute(&admin, "default", start.build(), "churn start backup").await;
    let mut started_result = start_response
        .results
        .shift_remove("churn-start")
        .unwrap_or_else(|| panic!("churn start backup: missing 'churn-start' alias"));
    assert!(
        !started_result.skipped,
        "churn start backup: cascade-skipped"
    );
    let started_record = started_result
        .records
        .pop()
        .unwrap_or_else(|| panic!("churn start backup: no records"));
    // The backup id up-front: every later window proof must match THIS run.
    let backup_id = started_record
        .get_value_str("backup_id")
        .unwrap_or_else(|| panic!("churn start backup: record missing 'backup_id'"))
        .to_owned();
    guard.mark("StartBackup sent");

    // (a-window) Wait for the FIRST ACTIVE SEAL COPY: from here the
    // captured set is fixed, so mid-window DDL is provably after capture.
    // Only a `Sealing`+running poll of THIS backup counts (the helper); a
    // `Verifying`/bookkeeping poll can never open the window.
    let window_deadline = overall_deadline.min(Instant::now() + Duration::from_secs(60));
    let first_progress: CopyProgress = loop {
        assert!(
            Instant::now() < window_deadline,
            "no active Sealing copy observed in time; the copy never reached the seal stage, \
             so churn-in-copy cannot be certified"
        );
        let status = poll_backup_status(&admin).await;
        let stage = stage_of(&status);
        assert!(
            stage != "Failed",
            "online backup reported stage Failed; status record: {status:?}"
        );
        assert!(
            status.get_value("last_error").is_none(),
            "online backup reported last_error: {:?}",
            status.get_value_str("last_error")
        );
        // The progress is captured only AFTER the response arrived.
        if let Some(progress) = active_seal_copy(&status, &backup_id) {
            guard.mark(format!(
                "live window opens (active Sealing copy, {} bytes hashed)",
                progress.bytes_hashed
            ));
            break progress;
        }
        assert!(
            running_of(&status),
            "backup finished ('{stage}') before any active Sealing copy was observed; the \
             churn window never opened"
        );
        tokio::task::yield_now().await;
    };
    guard.note("first active seal copy", format!("{first_progress:?}"));

    // (a-churn) The mid-copy catalogue churn, through the real typed
    // client: a SECOND PERSISTENT repo → its table → one durable write.
    let mut mk_repo = Batch::new();
    mk_repo.id("churn-mk-repo");
    mk_repo.create_repo(
        "churn-create-repo",
        ddl::create_repo(CHURN_REPO).engine(RepoEngine::Fjall),
    );
    run_step(&admin, "churn-create-repo", mk_repo).await;
    guard.mark("churn repo created during the live window");

    let mut mk_table = Batch::new();
    mk_table.id("churn-mk-table");
    mk_table.create_table(
        "churn-create-table",
        ddl::create_table(CHURN_TABLE).repo(CHURN_REPO),
    );
    run_step(&admin, "churn-create-table", mk_table).await;
    guard.mark("churn table created during the live window");

    let churn_acked_at;
    {
        let doc = write::doc()
            .set_value("writer", CHURN_WRITER)
            .set_value("seq", CHURN_SEQ)
            .set_value("value", CHURN_VALUE)
            .build();
        let mut batch = Batch::new();
        batch.id("churn-write");
        batch.transactional();
        batch.durability(Durability::Synced);
        batch.insert(
            CHURN_TABLE,
            write::Insert::with_repo(CHURN_REPO, CHURN_TABLE).row(doc),
        );
        let mut response = bounded_execute(&admin, DB, batch.build(), "churn-write").await;
        let result = response
            .results
            .shift_remove(CHURN_TABLE)
            .unwrap_or_else(|| panic!("churn-write: missing '{CHURN_TABLE}' alias"));
        assert!(!result.skipped, "churn-write: cascade-skipped");
        match response.transaction {
            Some(tx) if tx.is_committed() && tx.commit_version.is_some() => {}
            other => panic!("churn-write: not committed with a commit_version: {other:?}"),
        }
        churn_acked_at = Instant::now();
    }
    guard.mark("churn write acknowledged (durable)");

    // (a-overlap) At least one further ACTIVE SEAL COPY observed by a
    // request sent strictly AFTER the churn write's ack proves the copy was
    // still mid-seal while the churn write committed. Only `Sealing` polls
    // of THIS backup count (`active_seal_copy`) — `Verifying` and the final
    // bookkeeping stages can never satisfy the overlap proof. No fixed
    // sleeps.
    let mut last_progress: Option<CopyProgress> = None;
    let (final_status, _final_stage) = loop {
        assert!(
            Instant::now() < overall_deadline,
            "churn scenario exceeded the total budget waiting for Done; last proof of \
             overlap: {:?}",
            last_progress.is_some()
        );
        let sent_at = Instant::now();
        let status = poll_backup_status(&admin).await;
        let stage = stage_of(&status);
        assert!(
            stage != "Failed",
            "online backup reported stage Failed; status record: {status:?}"
        );
        assert!(
            status.get_value("last_error").is_none(),
            "online backup reported last_error: {:?}",
            status.get_value_str("last_error")
        );
        // The request must have STARTED strictly after the churn ack.
        if sent_at > churn_acked_at {
            if let Some(progress) = active_seal_copy(&status, &backup_id) {
                if last_progress.is_none() {
                    guard.mark(
                        "overlap proven: an active Sealing copy was observed after the churn ack",
                    );
                }
                last_progress = Some(progress);
            }
        }
        if !running_of(&status) {
            assert_eq!(
                stage, "Done",
                "finished backup must report Done (got '{stage}')"
            );
            guard.mark("backup Done");
            break (status, stage);
        }
        tokio::task::yield_now().await;
    };
    let last_progress = last_progress.unwrap_or_else(|| {
        panic!(
            "no active Sealing copy was observed after the churn write's ack: the copy may \
             have finished its seals before the churn committed — churn-in-copy is not certified"
        )
    });

    // The final totals are nonzero, and BOTH mid-copy polls were strictly
    // below them — proof each seal-copy observation was mid-copy, not after
    // completion.
    let final_progress = CopyProgress::from_status(&final_status)
        .unwrap_or_else(|| panic!("final Done status must carry progress.per_repo"));
    first_progress.assert_before_final(final_progress);
    last_progress.assert_before_final(final_progress);

    let dest_dir_str = final_status
        .get_value_str("dest_dir")
        .unwrap_or_else(|| panic!("final status: record missing 'dest_dir'"))
        .to_owned();
    let dest_dir = std::path::PathBuf::from(&dest_dir_str);
    assert_eq!(
        final_status
            .get_value_str("backup_id")
            .unwrap_or_else(|| panic!("final status: record missing 'backup_id'")),
        started_record
            .get_value_str("backup_id")
            .unwrap_or_else(|| panic!("start record missing 'backup_id'")),
        "final BackupStatus must report the started backup's id"
    );

    // ── graceful source shutdown BEFORE restore (fjall file locks) ──
    fixture::shutdown(admin, booted).await;
    guard.mark("source server shut down cleanly");
    assert!(
        Instant::now() < overall_deadline,
        "churn scenario exceeded the total budget before restore"
    );

    // (b) Captured set FIXED: the manifest's per-repo consistency section
    // is EXACTLY the pre-capture catalogue — the churn repo never enters.
    assert!(dest_dir.is_dir(), "backup dest_dir must exist after Done");
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
    // Bootstrap ALWAYS creates a persistent 'default' database with its
    // 'main' repo (launch.rs), so the captured set is 'default/main' plus
    // the fixture's 'backup_load/main' — never the mid-copy churn repo.
    let mut expected = std::collections::BTreeSet::new();
    expected.insert("default/main".to_owned());
    expected.insert(format!("{DB}/{REPO}"));
    let actual: std::collections::BTreeSet<String> = consistency.per_repo.keys().cloned().collect();
    assert_eq!(
        actual, expected,
        "the captured set must be exactly the pre-capture catalogue: the mid-copy churn \
         repo must not appear in the manifest"
    );
    let main_point = consistency
        .per_repo
        .get(&format!("{DB}/{REPO}"))
        .expect("per_repo must contain 'backup_load/main' (just checked)");
    assert!(
        main_point.wal_position >= 1,
        "manifest WAL position for '{DB}/{REPO}' must be set (got {})",
        main_point.wal_position
    );

    // (c) Clean-machine restore onto the ABSENT target, on a roomy thread.
    let restore_target = restored_data_dir.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("churn-restore".into())
        .stack_size(BIG_STACK)
        .spawn(move || {
            // force = false: clean-machine path (target absent).
            let _ = tx.send(shamir_server::restore::restore(
                &dest_dir,
                &restore_target,
                false,
            ));
        })
        .expect("spawn restore thread");
    let outcome = rx
        .recv_timeout(RESTORE_BOUND)
        .unwrap_or_else(|_| panic!("restore did not complete within {RESTORE_BOUND:?}"));
    guard.mark("restore thread finished");
    let report = outcome.unwrap_or_else(|e| panic!("restore failed: {e}"));
    assert!(
        report.files_restored > 0 && report.bytes_restored > 0,
        "restore must have restored real content: {} files / {} bytes",
        report.files_restored,
        report.bytes_restored
    );
    // Every exclusion is NAMED and explained: only the mid-copy churn repo
    // may appear (Q9); anything else is an unexplained exclusion.
    // Empty is equally legal (see the header's source-obstruction note).
    let allowed_exclusion = format!("{DB}/{CHURN_REPO}");
    for entry in &report.excluded_repositories {
        assert_eq!(
            *entry, allowed_exclusion,
            "unexplained restore exclusion (only '{allowed_exclusion}' may be named)"
        );
    }
    guard.note(
        "excluded_repositories",
        format!("{:?}", report.excluded_repositories),
    );

    // (c) Restored boot: the seeded `main` row is intact with its exact
    // value (the captured repo is consistent), the boot itself is clean —
    // and the mid-copy churn repo/table are INACCESSIBLE: a typed query
    // against them must fail with the server's not-found error.
    let restored_boot = fixture::boot(&restored_data_dir, false).await;
    let restored_client = fixture::connect(restored_boot.addr(), &restored_boot.password).await;
    let mut scan = Batch::new();
    scan.id("churn-restored-scan");
    scan.query(
        "churn-scan",
        Query::with_repo(REPO, TABLE)
            .select(["writer", "seq", "value"])
            .build(),
    );
    let mut response =
        bounded_execute(&restored_client, DB, scan.build(), "restored 'main' scan").await;
    let mut result = response
        .results
        .shift_remove("churn-scan")
        .unwrap_or_else(|| panic!("restored 'main' scan: missing 'churn-scan' alias"));
    assert!(!result.skipped, "restored 'main' scan: cascade-skipped");
    assert_eq!(
        result.records.len(),
        1,
        "the restored 'main' table must hold exactly the seeded row: got {} rows",
        result.records.len()
    );
    let row = result
        .records
        .pop()
        .expect("exactly one row (just checked)");
    assert_eq!(row.get_value_i64("writer"), Some(1), "seeded 'writer'");
    assert_eq!(row.get_value_i64("seq"), Some(1), "seeded 'seq'");
    assert_eq!(row.get_value_i64("value"), Some(7), "seeded 'value'");

    // The created-after-capture repo must NOT be registered by the restored
    // boot: a typed read of its table fails (server 'not found' error).
    let mut ghost = Batch::new();
    ghost.id("churn-ghost-query");
    ghost.query(
        "ghost",
        Query::with_repo(CHURN_REPO, CHURN_TABLE)
            .select(["writer", "seq", "value"])
            .build(),
    );
    match tokio::time::timeout(REQUEST_TIMEOUT, restored_client.execute(DB, ghost.build())).await {
        Ok(Err(shamir_client::ClientError::Db { message, .. })) => assert!(
            message.contains("not found"),
            "ghost churn repo query must fail as not-found, got: {message}"
        ),
        Ok(Err(other)) => {
            panic!("ghost churn repo query failed with an unexpected error kind: {other}")
        }
        Ok(Ok(response)) => panic!(
            "the churn repo must be inaccessible after restore, but the query succeeded: \
             aliases {:?}",
            response.results.keys().collect::<Vec<_>>()
        ),
        Err(_) => panic!("ghost churn repo query exceeded {REQUEST_TIMEOUT:?}"),
    }

    fixture::shutdown(restored_client, restored_boot).await;
    guard.mark("restored server shut down cleanly");
    assert!(
        Instant::now() < overall_deadline,
        "churn scenario exceeded the total budget"
    );
}

/// The churn test: the async scenario runs on a roomy 64 MiB thread with a
/// 16 MiB-per-worker runtime, under both the in-scenario timeout and the
/// [`TEST_BOUND`] watchdog — all under the 180 s nextest per-test kill.
#[test]
fn catalogue_churn_during_online_copy_restores_clean_with_named_exclusions_only() {
    let guard = FlakeGuard::with_watchdog(
        "online_backup churn: mid-copy repo + clean restore",
        TEST_BOUND,
    );
    guard.mark("churn test starts");
    let (tx, rx) = mpsc::channel::<Result<(), tokio::time::error::Elapsed>>();
    let handle = std::thread::Builder::new()
        .name("churn-main".into())
        .stack_size(BIG_STACK)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_stack_size(WORKER_STACK)
                .enable_all()
                .build()
                .expect("build the churn test runtime");
            let outcome = runtime
                .block_on(async { tokio::time::timeout(TEST_BOUND, churn_scenario(guard)).await });
            let _ = tx.send(outcome);
        })
        .expect("spawn the roomy churn thread");
    match rx.recv_timeout(TEST_BOUND + Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(_elapsed)) => panic!("churn scenario exceeded the overall {TEST_BOUND:?} timeout"),
        Err(_) => panic!(
            "churn thread panicked or did not finish within its bound ({} s)",
            (TEST_BOUND + Duration::from_secs(5)).as_secs()
        ),
    }
    handle
        .join()
        .expect("churn thread must not panic after reporting its outcome");
}
