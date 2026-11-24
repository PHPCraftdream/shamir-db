use super::*;

/// Attempts for the positive-budget claim. Between the end of the meta-snapshot
/// join (outer deadline minus 500 ms) and the start of the store-release probe
/// (outer deadline minus 250 ms) only ~250 ms remain for the reaper joins and
/// the synchronous drops of the engine and its stores. On a loaded runner that
/// stretch can run longer and the release stage then reports ZERO budget, a
/// different verdict (its text has its own test): the scenario is retried a
/// bounded number of times, and only on that complete verdict, instead of
/// asserting a slack the test cannot own.
const LATE_BUDGET_ATTEMPTS: usize = 3;

/// The reason the release stage writes when no budget was left.
const NO_BUDGET_VERDICT: &str = "no shutdown budget remained";

#[test]
fn late_teardown_stages_report_the_remaining_shutdown_budget() {
    try_install_global_tracing_ring();
    for attempt in 1..=LATE_BUDGET_ATTEMPTS {
        let (report, pool_still_occupied) = late_budget_attempt(attempt);
        // Everything that does not depend on the slack is checked on EVERY
        // attempt: a deviation there is never a reason to retry.
        assert_common_claims(&report, pool_still_occupied);
        if attempt < LATE_BUDGET_ATTEMPTS && is_zero_budget_verdict(&report) {
            eprintln!(
                "late-budget attempt {attempt}: the release stage reported zero budget, \
                 running another attempt"
            );
            continue;
        }
        assert_positive_budget_verdict(&report);
        return;
    }
}

/// One attempt: a fresh runtime with ONE blocking thread (dropped, and with it
/// every queued blocking job drained, before the data directory is removed).
fn late_budget_attempt(attempt: usize) -> (shamir_db::ShutdownReport, bool) {
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    create_private_dir(&data_dir).expect("private data directory");
    let outcome = run_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .expect("test runtime")
            .block_on(async {
                timeout(BOUND, late_budget_shutdown(attempt, data_dir))
                    .await
                    .expect("the shutdown must settle inside the bound even with a starved pool")
            })
    });
    drop(temp);
    outcome
}

/// One real launch and one deadline shutdown with the blocking pool parked.
async fn late_budget_shutdown(
    attempt: usize,
    data_dir: PathBuf,
) -> (shamir_db::ShutdownReport, bool) {
    let guard = FlakeGuard::with_watchdog(
        "late_teardown_stages_report_the_remaining_shutdown_budget",
        Duration::from_secs(300),
    );
    guard.note("attempt", attempt.to_string());

    // No backup schedule: the scheduled worker's wait is a separate stage, and
    // this test is about the stages AFTER the audit and repository teardown.
    let handle = ServerLauncher {
        config: bind_config(data_dir, "127.0.0.1:0".to_owned(), "127.0.0.1:0".to_owned()),
        bootstrap: BootstrapMode::Skip,
        node_mode: NodeMode::ReadWrite,
    }
    .launch()
    .await
    .expect("launch real server");
    guard.mark("launched");

    // Occupy the ONLY blocking thread for the whole shutdown. The job parks on
    // a channel the test releases after the report is in hand — never a sleep.
    let occupant = BlockingPoolOccupant::park();
    occupant.await_started();
    guard.mark("blocking pool occupied");

    let report = crate::runtime::drain_server(handle, REMAINING_BUDGET_DEADLINE).await;
    let pool_still_occupied = !occupant.finished();
    guard.mark("shutdown returned with the blocking pool still occupied");

    // Unconditional release before any assertion: a failing claim must never
    // leave a parked job behind (`Drop` releases it too).
    drop(occupant);
    (report, pool_still_occupied)
}

/// The full zero-budget store-release verdict (not any line that merely names
/// the reason).
fn is_zero_budget_verdict(report: &shamir_db::ShutdownReport) -> bool {
    report.errors().iter().any(|error| {
        error.contains("store release could not be verified")
            && error.contains(NO_BUDGET_VERDICT)
            && error.contains("no probe was started")
            && error.contains("UNKNOWN")
    })
}

fn assert_common_claims(report: &shamir_db::ShutdownReport, pool_still_occupied: bool) {
    let errors = report.errors();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("meta snapshot task did not stop within")),
        "the meta-snapshot join must report the snapshot it could not settle while the \
         blocking pool is parked (mutant: the unbounded join never reaches step 7): {report:?}"
    );
    assert!(
        pool_still_occupied,
        "the report must come back while the blocking pool is still parked — the late stages \
         wait for their BUDGET, not for the pool: {report:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|error| error.contains("graceful shutdown exceeded deadline")),
        "the late stages must settle INSIDE the outer deadline, so the generic expiry message \
         must not stand in for their verdict: {report:?}"
    );
    assert!(
        !report.is_clean(),
        "a teardown whose late stages could not settle is unclean: {report:?}"
    );
    assert_eq!(report.exit_code(), 70, "unclean report: {report:?}");
}

fn assert_positive_budget_verdict(report: &shamir_db::ShutdownReport) {
    let errors = report.errors();
    let Some(store_line) = errors
        .iter()
        .find(|error| error.contains("store release could not be verified within "))
    else {
        panic!(
            "the store-release stage must reach the report with its own verdict (mutant: \
             `release_stores(&data_dir, STORE_RELEASE_TIMEOUT)` parks for the whole 10 s \
             constant, the outer deadline expires inside it, and only the generic message \
             remains): {report:?}"
        );
    };
    assert!(
        store_line.contains("store release could not be verified within ")
            && store_line.contains("the store-release blocking job did not complete before its deadline")
            && store_line.contains("the blocking job may still have been queued or running")
            && store_line.contains("UNKNOWN")
            && !store_line.contains(NO_BUDGET_VERDICT)
            && !store_line.contains("no probe was started")
            && !store_line.contains("within 0ns")
            && !store_line.contains("10s"),
        "the store-release line must report the positive probe budget and pool-starved result, not the 10 s constant: \
         {store_line}"
    );
}
