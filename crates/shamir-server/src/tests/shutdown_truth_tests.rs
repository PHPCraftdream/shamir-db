use std::future::pending;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::codecs::interned::query_value_to_inner;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig, RepoInstance};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::tx::IsolationLevel;
use shamir_db::{ShamirDb, ShutdownReport};
use shamir_query_builder::write::doc;
use tracing::Level;

use crate::runtime::finish_shutdown;

// Red before SHUTDOWN-TRUTH: shutdown errors and deadlines were success-shaped.
#[tokio::test(start_paused = true)]
async fn deadline_preserves_completed_repository_outcomes() {
    let mut report = ShutdownReport::pending(vec!["app/good".into(), "app/bad".into()]);
    report.record("app/good", Ok(()));
    report.record("app/bad", Err("injected flush failure".into()));
    let report = finish_shutdown(
        Duration::from_secs(30),
        &mut report,
        |_| Box::pin(pending()),
    )
    .await;
    assert!(!report.is_clean());
    assert_eq!(report.exit_code(), 70);
    assert_eq!(
        report.repository_failures(),
        vec![("app/bad", "injected flush failure")],
    );
    assert!(report.errors().iter().any(|e| e.contains("deadline")));
}

#[tokio::test(start_paused = true)]
async fn deadline_names_only_unfinished_repositories() {
    let mut report = ShutdownReport::pending(vec!["app/done".into(), "app/parked".into()]);
    report.record("app/done", Ok(()));
    let report = finish_shutdown(
        Duration::from_secs(30),
        &mut report,
        |_| Box::pin(pending()),
    )
    .await;
    assert_eq!(
        report.repository_failures(),
        vec![("app/parked", "shutdown did not complete")],
    );
    assert_eq!(report.exit_code(), 70);
}

#[test]
fn unclean_shutdown_increments_counter_once_and_clean_does_not() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let clean = ShutdownReport::pending(Vec::new());
        crate::observability::record_shutdown(&clean);
        assert_eq!(clean.exit_code(), 0);
        let mut report = ShutdownReport::pending(vec!["app/good".into(), "app/bad".into()]);
        report.record("app/good", Ok(()));
        report.record("app/bad", Err("injected flush failure".into()));
        crate::observability::record_shutdown(&report);
        assert_eq!(
            report.repository_failures(),
            vec![("app/bad", "injected flush failure")],
        );
        assert_eq!(report.exit_code(), 70);
    });
    assert!(handle
        .render()
        .lines()
        .any(|line| line == "shutdown_unclean_total 1"));
}

// Real checkpoint failure: stopping only the journal writer makes J submission
// fail while the sibling still reaches its clean horizon.
#[tokio::test]
async fn failed_checkpoint_names_only_failed_repository() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("app").await;
    for name in ["good", "bad"] {
        db.add_repo(RepoConfig::new(name, BoxRepoFactory::in_memory()))
            .await
            .unwrap();
    }
    let bad = db.get_repo("bad").unwrap();
    bad.changefeed()
        .await
        .unwrap()
        .feed
        .shutdown_and_wait()
        .await
        .unwrap();
    let mut report = shamir.shutdown_report();
    shamir
        .flush_all_report(Duration::from_secs(5), &mut report)
        .await;
    assert!(!report.is_clean());
    let failures = report.repository_failures();
    assert_eq!(failures.len(), 1, "{report:?}");
    assert_eq!(failures[0].0, "app/bad");
    assert_eq!(
        failures[0].1,
        "Storage backend error: changefeed writer closed",
    );
    assert_eq!(
        db.get_repo("good")
            .unwrap()
            .changefeed()
            .await
            .unwrap()
            .feed
            .clean_horizon(),
        0,
    );
    assert_eq!(report.exit_code(), 70);
    assert_shutdown_error_and_metric(&report, "changefeed writer closed");
}

#[tokio::test(start_paused = true)]
async fn shared_budget_exhaustion_is_unclean() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("app").await;
    db.add_repo(RepoConfig::new("parked", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let mut report = shamir.shutdown_report();
    shamir.flush_all_report(Duration::ZERO, &mut report).await;
    assert!(!report.is_clean());
    assert!(report
        .repository_failures()
        .iter()
        .any(|(name, error)| { *name == "app/parked" && error.contains("timed out") }));
    assert_eq!(report.exit_code(), 70);
}

#[test]
fn clean_report_writes_j_and_hard_reopen_certifies_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let h = runtime.block_on(async {
        let shamir = ShamirDb::init_memory().await.unwrap();
        let db = shamir.create_db("app").await;
        db.add_repo(
            RepoConfig::new("clean", BoxRepoFactory::fjall_raw(&path))
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
        let repo = db.get_repo("clean").unwrap();
        let gate = repo.tx_gate().await.unwrap();
        let table = repo.get_table("items").await.unwrap();
        let interner = table.interner().get().await.unwrap();
        let value = query_value_to_inner(&doc().set("name", "durable").build(), interner).unwrap();
        let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        table.insert_tx(&value, Some(&mut tx)).await.unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(snapshot);
        let h = gate.version_allocation_high_water_mark();
        assert!(h > 0);
        let mut report = shamir.shutdown_report();
        shamir
            .flush_all_report(Duration::from_secs(5), &mut report)
            .await;
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.exit_code(), 0);
        assert_eq!(repo.changefeed().await.unwrap().feed.clean_horizon(), h);
        h
    });
    drop(runtime);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let repo = RepoInstance::from_factory(
            "clean".into(),
            BoxRepoFactory::fjall_raw(&path),
            vec![TableConfig::new("items")],
        )
        .await
        .unwrap();
        assert_eq!(repo.changefeed().await.unwrap().feed.clean_horizon(), h);
        assert_eq!(repo.read_changelog_from(0, 100).await.unwrap().gap_at, None);
    });
    drop(runtime);
}

#[derive(Clone)]
struct ShutdownLog(Arc<Mutex<Vec<u8>>>);

impl Write for ShutdownLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Test-local capture only; no guard crosses an await.
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// Catches duplicate ERROR emission, wrong severity, missing repository context,
// and logging a healthy repository as failed. No global subscriber is installed.
#[test]
fn unclean_shutdown_emits_one_error_and_clean_emits_none() {
    let mut report = ShutdownReport::pending(vec!["app/good".into(), "app/bad".into()]);
    report.record("app/good", Ok(()));
    report.record("app/bad", Err("injected flush failure".into()));
    assert_shutdown_error_and_metric(&report, "injected flush failure");
}

fn assert_shutdown_error_and_metric(report: &ShutdownReport, error: &str) {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let output = ShutdownLog(Arc::new(Mutex::new(Vec::new())));
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::ERROR)
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("shutdown_unclean_total").increment(0);
            crate::observability::record_shutdown(&ShutdownReport::pending(Vec::new()));
            assert!(output.0.lock().unwrap().is_empty());
            assert!(handle
                .render()
                .lines()
                .any(|line| line == "shutdown_unclean_total 0"));
            crate::observability::record_shutdown(report);
        });
    });
    assert!(handle
        .render()
        .lines()
        .any(|line| line == "shutdown_unclean_total 1"));
    let captured = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(captured.lines().count(), 1, "{captured}");
    assert!(captured.contains("ERROR"), "{captured}");
    assert!(captured.contains("app/bad"), "{captured}");
    assert!(captured.contains(error), "{captured}");
    assert!(!captured.contains("app/good"), "{captured}");
    assert!(
        captured.contains("next start will certify a conservative replication gap"),
        "{captured}",
    );
}
