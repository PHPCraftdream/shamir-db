//! The alert RETENTION-failure input, on the real `/metrics` route: one
//! obstructed retention pass.

use std::fs;
use std::sync::Arc;

use shamir_private_dir::create_private_dir;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::boot::config::diagnostics::backup_online::metrics::backup_metrics_snapshot;
use crate::boot::config::diagnostics::backup_online::registry::arm_scheduled_retention_end;
use crate::boot::config::diagnostics::backup_online::schedule::runner::spawn_for_test;
use crate::boot::config::diagnostics::backup_online::OnlineBackupRegistry;
use crate::server::{BootstrapMode, ServerLauncher};
use crate::sync::durability::backup::online::set_snapshot_name_base_seconds;
use crate::sync::durability::backup::SNAPSHOT_LEASE_FILE_NAME;

use super::support::{
    metric_type, obstruct_lease_marker, on_big_stack, sample, scheduled_config, scrape_metrics,
    TerminalOutcome, BOUND, OUTER, SNAPSHOT_NAME_BASE, TICK, WATCHDOG,
};

const ALERT_RETENTION_TEST: &str = "live_alert_retention_failure_reaches_the_real_metrics_route";

#[test]
fn live_alert_retention_failure_reaches_the_real_metrics_route() {
    on_big_stack(|| async {
        timeout(OUTER, run_alert_retention())
            .await
            .expect("live alert-retention acceptance timed out");
    });
}

/// Arm the NEXT run's runner-thread observer: its post-run callback installs
/// the retention-END seam on that run's OWN runner thread, so the END fires
/// after the pass published its counters. Re-armed before the next admission.
fn arm_retention_end_observer(registry: &OnlineBackupRegistry, ends: &mpsc::UnboundedSender<()>) {
    let ends = ends.clone();
    registry.arm_post_run_callback_for_shutdown_test(move |_snapshot| {
        let ends = ends.clone();
        let end = arm_scheduled_retention_end(move || {
            let _ = ends.send(());
        });
        // The hook fires in THIS run's retention pass; the zero-sized guard
        // would only clear an unfired install.
        std::mem::forget(end);
        None
    });
}

async fn run_alert_retention() {
    let guard = FlakeGuard::with_watchdog(ALERT_RETENTION_TEST, WATCHDOG);
    try_install_global_tracing_ring();
    set_snapshot_name_base_seconds(SNAPSHOT_NAME_BASE);
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    let backups = temp.path().join("backups");
    create_private_dir(&data_dir).expect("private data directory");
    create_private_dir(&backups).expect("private backup directory");

    // Nextest gives this test its own process: the pre-run zero.
    let baseline = backup_metrics_snapshot();
    assert_eq!(
        baseline.last_success_timestamp_seconds, 0,
        "a fresh test process must not have published a success yet"
    );

    let (send, mut receive) = mpsc::unbounded_channel::<TerminalOutcome>();
    let (retention_end_tx, mut retention_end_rx) = mpsc::unbounded_channel::<()>();
    let handle = ServerLauncher::new(scheduled_config(&data_dir, &backups, "1h", true, 2))
        .with_bootstrap(BootstrapMode::Skip)
        .launch_with_backup_test(move |resolved, registry, owner, shutdown| {
            let scheduler_shutdown = shutdown.child_token();
            // Retention-END handshake for run 1; the outcome observer re-arms
            // it for every following run.
            arm_retention_end_observer(&registry, &retention_end_tx);
            let observer_registry = Arc::clone(&registry);
            let observer_ends = retention_end_tx.clone();
            spawn_for_test(
                resolved,
                registry,
                owner,
                scheduler_shutdown,
                TICK,
                move |ok| {
                    arm_retention_end_observer(&observer_registry, &observer_ends);
                    let _ = send.send(ok);
                },
            )
        })
        .await
        .expect("launch a real server with the scheduled worker");
    guard.mark("server launched with a millisecond tick");
    let addr = handle
        .observability
        .as_ref()
        .expect("observability")
        .bound_addr;

    // A FORCED retention failure at the registry level: a DIRECTORY at the
    // lease-marker path, so provisioning fails while the copy still succeeds.
    let marker = backups.join(SNAPSHOT_LEASE_FILE_NAME);
    let marker_meta = fs::symlink_metadata(&marker).expect("lease marker provisioned at boot");
    assert!(marker_meta.is_file(), "lease marker must be a regular file");
    obstruct_lease_marker(&mut receive, &backups, &marker).await;
    guard.mark("lease marker obstructed");
    // Drain already-delivered outcomes: every run ADMITTED from here on
    // reports a retention failure.
    while receive.try_recv().is_ok() {}
    let retention_before = backup_metrics_snapshot().retention_failed_total;
    // Wait for the EVENT, not for runs: each iteration waits for a real
    // retention END, never a sleep, so a pass that started before the
    // obstruction cannot be mistaken for the obstructed one.
    let deadline = tokio::time::Instant::now() + BOUND;
    loop {
        match tokio::time::timeout_at(deadline, retention_end_rx.recv()).await {
            Ok(Some(())) => {
                if backup_metrics_snapshot().retention_failed_total > retention_before {
                    break;
                }
            }
            Ok(None) => panic!("the scheduled worker stopped before the obstructed retention pass"),
            Err(_) => panic!("an obstructed retention pass must end within the bound"),
        }
    }
    guard.mark("obstructed retention pass ended");
    let settled_retention = backup_metrics_snapshot().retention_failed_total;
    assert!(
        settled_retention > retention_before,
        "a marker-provisioning failure must be accounted as a retention failure \
         (mutant: `MarkerProvisioning::Failed` publishing no failure): \
         before={retention_before} after={settled_retention}"
    );
    let route = scrape_metrics(addr).await;
    let route_value = sample(&route, "shamir_backup_retention_failed_total");
    assert!(
        route_value > baseline.retention_failed_total,
        "the retention-failure counter must ADVANCE on the real route (mutant: a \
         counter that never advances): before={} route={route_value}",
        baseline.retention_failed_total
    );
    assert!(
        route_value >= settled_retention,
        "the route must render the process counter monotonically: \
         route={route_value} observed={settled_retention}"
    );
    assert_eq!(
        metric_type(&route, "shamir_backup_retention_failed_total"),
        "counter"
    );

    let report = timeout(BOUND, handle.shutdown())
        .await
        .expect("shutdown must finish within the bound");
    guard.mark("server stopped");
    assert!(
        report.is_clean(),
        "a server whose scheduled worker finished must shut down cleanly: {report:?}"
    );
}
