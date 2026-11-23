//! Real-server acceptance for the SCHEDULED backup lifecycle: the PRODUCTION
//! spawn/join path and its token binding, the alert inputs on the real
//! `/metrics` route, and the retained/pruned destination.
//!
//! Nothing synchronises with a sleep: every wait is bounded by [`BOUND`] and
//! observes the scheduler's or the retention pass's OWN signal.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::panic::resume_unwind;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use shamir_private_dir::create_private_dir;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Builder;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::boot::config::diagnostics::backup_online::metrics::backup_metrics_snapshot;
use crate::boot::config::diagnostics::backup_online::registry::{
    arm_scheduled_retention_end, arm_scheduled_retention_pause,
};
use crate::boot::config::diagnostics::backup_online::schedule::runner::{spawn, spawn_for_test};
use crate::boot::config::diagnostics::backup_online::OnlineBackupRegistry;
use crate::boot::config::HttpListenerSettings;
use crate::config::{Config, ListenerConfig, ListenerKind, ResolvedSchedule, TlsConfig};
use crate::server::{BootstrapMode, ServerLauncher};
use crate::sync::durability::backup::online::set_snapshot_name_base_seconds;
use crate::sync::durability::backup::{
    verify_manifest, MANIFEST_FILE_NAME, SNAPSHOT_LEASE_FILE_NAME,
};

const TEST_NAME: &str = "scheduled_backups_verify_prune_and_export_metrics_on_a_real_server";
const PRODUCTION_SPAWN_TEST: &str =
    "production_spawn_joins_the_parked_scheduler_and_binds_the_root_token";
const ALERT_INPUTS_TEST: &str =
    "live_alert_inputs_track_the_scheduled_lifecycle_on_the_real_metrics_route";
const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../deploy/server.example.ktav"
));

/// Injected tick spacing: MILLISECONDS, so no assertion waits on a second.
const TICK: Duration = Duration::from_millis(40);
/// Fixed base second of the injected snapshot-name clock: every run gets
/// `base + n`, so no two runs collide on the `<to>/<timestamp>` create.
const SNAPSHOT_NAME_BASE: u64 = 1_700_000_000;
/// One generous bound for every wait (the Windows CI runner is up to 10x slower).
const BOUND: Duration = Duration::from_secs(120);
/// How long the shutdown future is watched while the worker is parked: a
/// launcher that stopped awaiting the scheduled task returns within it.
const PENDING_WINDOW: Duration = Duration::from_secs(5);
/// Short negative probe window for the retention park (see `run_retention_park`).
const PARKED_PROBE: Duration = Duration::from_millis(500);

#[test]
fn scheduled_backups_verify_prune_and_export_metrics_on_a_real_server() {
    on_big_stack(|| async {
        timeout(Duration::from_secs(180), run())
            .await
            .expect("scheduled backup acceptance timed out");
    });
}

async fn run() {
    let guard = FlakeGuard::with_watchdog(TEST_NAME, Duration::from_secs(150));
    try_install_global_tracing_ring();
    set_snapshot_name_base_seconds(SNAPSHOT_NAME_BASE);
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    let backups = temp.path().join("backups");
    create_private_dir(&data_dir).expect("private data directory");
    create_private_dir(&backups).expect("private backup directory");

    let foreign = backups.join("foreign");
    let invalid = backups.join("20200101_000001");
    fs::create_dir(&foreign).expect("foreign directory");
    fs::create_dir(&invalid).expect("invalid snapshot directory");
    let foreign_payload = foreign.join("keep.txt");
    let invalid_manifest = invalid.join(MANIFEST_FILE_NAME);
    fs::write(&foreign_payload, b"foreign payload").expect("foreign payload");
    fs::write(&invalid_manifest, b"invalid manifest").expect("invalid manifest");
    assert!(verify_manifest(&invalid).is_err());

    let config = scheduled_config(&data_dir, &backups, "1m", true, 2);

    let baseline = backup_metrics_snapshot();
    let (send, mut receive) = mpsc::unbounded_channel();
    let (published_tx, mut published_rx) = mpsc::unbounded_channel::<OsString>();
    let (retention_end_tx, mut retention_end_rx) = mpsc::unbounded_channel::<()>();
    let handle = ServerLauncher::new(config)
        .with_bootstrap(BootstrapMode::Skip)
        .launch_with_backup_test(|resolved, registry, owner, shutdown| {
            // Boot provisioning (R2d-FIX-C) happens BEFORE the schedule
            // spawn: the marker must already be a regular file here.
            let boot_marker = resolved.destination.join(SNAPSHOT_LEASE_FILE_NAME);
            let marker_meta =
                fs::symlink_metadata(&boot_marker).expect("lease marker provisioned at boot");
            assert!(
                marker_meta.is_file(),
                "lease marker must be a regular file, not a link or directory"
            );
            let scheduler_shutdown = shutdown.child_token();
            // Retention-END handshake + published-name reporting for run 1;
            // the outcome observer re-arms it for every following run.
            arm_run_observer(&registry, &published_tx, &retention_end_tx);
            let observer_registry = Arc::clone(&registry);
            let observer_published = published_tx.clone();
            let observer_ends = retention_end_tx.clone();
            let mut successes = 0;
            // The schedule's own interval is replaced by the injected
            // millisecond tick: the fixture never waits for a wall-clock
            // second, and each run is awaited through its terminal signal.
            spawn_for_test(
                resolved,
                registry,
                owner,
                scheduler_shutdown.clone(),
                TICK,
                move |ok| {
                    arm_run_observer(&observer_registry, &observer_published, &observer_ends);
                    successes += usize::from(ok);
                    if successes == 3 {
                        scheduler_shutdown.cancel();
                    }
                    // Unbounded: a dropped outcome would hang the wait below.
                    let _ = send.send(ok);
                },
            )
        })
        .await
        .expect("launch real server with backup scheduler");
    guard.mark("server launched");

    let addr = handle
        .observability
        .as_ref()
        .expect("observability")
        .bound_addr;
    // The self-observed "run N-1" stamp; no `SystemTime::now()` oracle.
    let mut first_success_stamp = 0;
    for index in 0..3 {
        let outcome = timeout(BOUND, receive.recv())
            .await
            .expect("every scheduled terminal signal must arrive within the bound")
            .expect("the scheduled worker must keep its terminal channel open");
        assert!(outcome, "scheduled copy must succeed");
        if index == 0 {
            first_success_stamp = sample(
                &scrape_metrics(addr).await,
                "shamir_backup_last_success_timestamp_seconds",
            );
            assert!(
                first_success_stamp > 0,
                "the first scheduled success must stamp a nonzero last-success value"
            );
        }
        guard.mark("scheduled copy outcome reported (retention follows)");
    }
    // Runs are admitted only after the previous run's `finished` (retention
    // included), so the THIRD retention END is the LAST run's and everything
    // below observes the settled retention. No sleep, no shutdown handoff; the
    // published names arrive in run order.
    let mut snapshots = Vec::with_capacity(3);
    for _ in 0..3 {
        snapshots.push(
            timeout(BOUND, published_rx.recv())
                .await
                .expect("every published snapshot name must arrive within the bound")
                .expect("the run observer channel must stay open"),
        );
        timeout(BOUND, retention_end_rx.recv())
            .await
            .expect("every retention pass must end within the bound")
            .expect("the retention-end channel must stay open");
    }
    let metrics = scrape_metrics(addr).await;
    assert_eq!(
        sample(&metrics, "shamir_backup_retention_deleted_total"),
        baseline.retention_deleted_total + 1,
        "exactly the one snapshot beyond keep_last = 2 must be deleted"
    );
    assert_eq!(
        sample(&metrics, "shamir_backup_retention_untouched"),
        2,
        "the foreign and invalid entries must be counted untouched on the real route"
    );

    let completed = sample(
        &metrics,
        "shamir_backup_scheduled_runs_total{result=\"completed\"}",
    );
    assert!(completed >= baseline.scheduled_runs_total[4] + 3);
    // Advancement, not a wall-clock comparison: self-observed stamps only.
    let last_success = sample(&metrics, "shamir_backup_last_success_timestamp_seconds");
    assert!(last_success > 0);
    assert!(
        last_success >= first_success_stamp,
        "the last-success stamp must not regress below the value observed after the first run \
         (self-observed values, no wall-clock oracle): first={first_success_stamp} \
         last={last_success}"
    );
    assert!(sample(&metrics, "shamir_backup_last_bytes") > 0);

    assert_eq!(
        fs::read(&foreign_payload).expect("foreign payload"),
        b"foreign payload"
    );
    assert_eq!(
        fs::read(&invalid_manifest).expect("invalid manifest"),
        b"invalid manifest"
    );
    // Filesystem: only the last `keep_last = 2` snapshots survive.
    let non_snapshots = [
        foreign.file_name().expect("foreign name").to_owned(),
        invalid.file_name().expect("invalid name").to_owned(),
        OsString::from(SNAPSHOT_LEASE_FILE_NAME),
    ];
    let retained: BTreeSet<OsString> = fs::read_dir(&backups)
        .expect("read backup root")
        .map(|entry| entry.expect("backup entry").file_name())
        .filter(|name| !non_snapshots.contains(name))
        .collect();
    assert_eq!(
        retained,
        BTreeSet::from([snapshots[1].clone(), snapshots[2].clone()]),
        "exactly the last keep_last = 2 scheduled snapshots must survive"
    );
    assert!(
        !backups.join(&snapshots[0]).exists(),
        "the oldest scheduled snapshot must be pruned"
    );
    for name in &snapshots[1..] {
        verify_manifest(&backups.join(name)).expect("retained snapshot verifies");
    }

    handle.shutdown().await;
    guard.mark("server stopped");
}

/// A real scheduled run's outcome, as delivered by the runner's terminal
/// signal. `true` is `Completed`, `false` is `Failed`.
type TerminalOutcome = bool;

/// Arm the NEXT run's runner-thread observer: its post-run callback runs on
/// that run's OWN runner thread, after the copy and before retention, so it
/// installs the retention-END seam there (the pause seam's counterpart) and
/// reports the published name. Re-armed before the next admission, which waits
/// for this run's `finished` (sent after retention).
fn arm_run_observer(
    registry: &OnlineBackupRegistry,
    published: &mpsc::UnboundedSender<OsString>,
    retention_ends: &mpsc::UnboundedSender<()>,
) {
    let published = published.clone();
    let ends = retention_ends.clone();
    registry.arm_post_run_callback_for_shutdown_test(move |snapshot| {
        let ends = ends.clone();
        if let Some(name) = snapshot.file_name() {
            let _ = published.send(name.to_owned());
        }
        let end = arm_scheduled_retention_end(move || {
            let _ = ends.send(());
        });
        // The hook fires in THIS run's retention pass; the zero-sized guard
        // would only clear an unfired install.
        std::mem::forget(end);
        None
    });
}

/// Wait (bounded) for a terminal signal that matches `expected`; stale
/// outcomes from a previous phase are consumed and skipped. No sleep, no poll:
/// each iteration awaits the scheduler's own signal.
async fn await_outcome(
    receive: &mut mpsc::UnboundedReceiver<TerminalOutcome>,
    expected: TerminalOutcome,
    stage: &str,
) {
    let deadline = tokio::time::Instant::now() + BOUND;
    loop {
        match tokio::time::timeout_at(deadline, receive.recv()).await {
            Ok(Some(outcome)) if outcome == expected => return,
            Ok(Some(_)) => {}
            Ok(None) => panic!("the scheduled worker stopped before {stage}"),
            Err(_) => panic!("{stage} did not arrive within {BOUND:?}"),
        }
    }
}

/// The exporter's TYPE line for one series family (`# TYPE <name> <kind>`).
fn metric_type<'a>(metrics: &'a str, series: &str) -> &'a str {
    metrics
        .lines()
        .find_map(|line| {
            line.strip_prefix("# TYPE ")?
                .strip_prefix(series)?
                .strip_prefix(' ')
        })
        .unwrap_or_else(|| panic!("missing /metrics TYPE line for {series}"))
        .trim()
}

/// Install the registry-level retention-failure injection: a DIRECTORY at the
/// lease-marker path, so `provision_snapshot_lease` fails for every run from
/// now on. The plain marker is MOVED ASIDE (a delete of a file a retention
/// pass holds open stays pending on Windows). Each retry waits for the next
/// terminal outcome — a real run boundary — bounded by [`BOUND`].
async fn obstruct_lease_marker(
    receive: &mut mpsc::UnboundedReceiver<TerminalOutcome>,
    root: &Path,
    marker: &Path,
) {
    let deadline = tokio::time::Instant::now() + BOUND;
    for attempt in 0..64 {
        // A fresh name per attempt: replacing an ALREADY PARKED marker that a
        // retention pass still holds open is not portable, moving a NEW one is.
        let parked = root.join(format!("lease-marker-parked-{attempt}"));
        match fs::rename(marker, &parked) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("move the lease marker aside: {error}"),
        }
        match fs::create_dir(marker) {
            Ok(()) => return,
            // A scheduled run re-provisioned the plain marker first: wait for
            // that run's boundary, then retry.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("obstruct the lease marker: {error}"),
        }
        match tokio::time::timeout_at(deadline, receive.recv()).await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the scheduled worker stopped while obstructing the lease marker"),
            Err(_) => panic!("could not obstruct the lease marker within {BOUND:?}"),
        }
    }
    panic!("could not obstruct the lease marker: a scheduled run kept re-provisioning it");
}

/// A real server whose scheduled backup runs into `backups`; the interval only
/// has to parse (ticking tests inject a millisecond interval via `spawn_for_test`).
fn scheduled_config(
    data_dir: &Path,
    backups: &Path,
    interval: &str,
    run_on_start: bool,
    keep_last: usize,
) -> Config {
    let text = format!(
        "{EXAMPLE}\nbackup: {{\ndestination: backups\ninterval: {interval}\nrun_on_start: {run_on_start}\nretention: {{\nkeep_last: {keep_last}\n}}\n}}\n"
    );
    let mut config: Config = ktav::from_str(&text).expect("server KTAV with backup schedule");
    config.backup.as_mut().expect("backup schedule").destination = backups.to_path_buf();
    config.tls = TlsConfig {
        cert_path: data_dir.join("cert.pem"),
        key_path: data_dir.join("key.pem"),
    };
    config.data_dir = data_dir.to_path_buf();
    config.listeners = vec![ListenerConfig {
        kind: ListenerKind::Http,
        addr: "127.0.0.1:0".into(),
        profile: None,
        http: HttpListenerSettings {
            tls: Some(false),
            ..Default::default()
        },
        path: None,
        kdf_override: None,
        browser_origin_allowlist: Vec::new(),
    }];
    config.observability.addr = "127.0.0.1:0".into();
    config.observability.allow_public_metrics = false;
    config
}

// ---------------------------------------------------------------------------
// (a) PRODUCTION spawn path + PRODUCTION cancellation token
// ---------------------------------------------------------------------------

/// The worker-side rendezvous state of the parked scheduled run.
#[derive(Clone, Copy, Default)]
struct WorkerGate {
    released: bool,
}

/// Block the runner thread until the test releases the gate. Bounded: a test
/// that never releases fails instead of hanging the suite.
fn wait_for_release(gate: &(Mutex<WorkerGate>, Condvar)) {
    let (lock, condvar) = gate;
    let deadline = Instant::now() + BOUND;
    let mut state = lock.lock().expect("worker gate");
    while !state.released {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (next, wait) = condvar
            .wait_timeout(state, remaining)
            .expect("worker gate wait");
        state = next;
        if wait.timed_out() && !state.released {
            panic!("the parked scheduled worker was not released within {BOUND:?}");
        }
    }
}

fn release_worker(gate: &(Mutex<WorkerGate>, Condvar)) {
    let (lock, condvar) = gate;
    lock.lock().expect("worker gate").released = true;
    condvar.notify_all();
}

/// State the test hands to the launcher's spawn position. The spawn argument
/// is a plain FUNCTION ITEM, so the state travels through this slot.
struct ProductionSpawnBridge {
    /// Signalled from the runner thread once the real run is parked.
    parked: Arc<Semaphore>,
    /// Released by the test; the runner thread blocks on it.
    gate: Arc<(Mutex<WorkerGate>, Condvar)>,
    /// Set by the runner thread AFTER the release: the worker's cleanup is
    /// observable only through this flag.
    resumed: Arc<AtomicBool>,
    /// The registry the launcher created (a run's cancel flag derives from
    /// the launcher's root token, not from any token of the test's).
    registry: Mutex<Option<Arc<OnlineBackupRegistry>>>,
}

static PRODUCTION_SPAWN_BRIDGE: Mutex<Option<Arc<ProductionSpawnBridge>>> = Mutex::new(None);

/// The `launch_with_backup_test` spawn argument: record the launcher's registry,
/// arm the one-shot post-run park on it, then call the PRODUCTION
/// `schedule::runner::spawn` with the launcher's four arguments unchanged —
/// only the arming step is inserted, because it needs the registry.
fn arm_then_spawn_production_worker(
    schedule: ResolvedSchedule,
    registry: Arc<OnlineBackupRegistry>,
    owner: String,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let bridge = PRODUCTION_SPAWN_BRIDGE
        .lock()
        .expect("production spawn bridge")
        .clone()
        .expect("the test installs the bridge before launching");
    *bridge.registry.lock().expect("registry slot") = Some(Arc::clone(&registry));
    let parked = Arc::clone(&bridge.parked);
    let gate = Arc::clone(&bridge.gate);
    let resumed = Arc::clone(&bridge.resumed);
    registry.arm_post_run_callback_for_shutdown_test(move |_published| {
        parked.add_permits(1);
        wait_for_release(&gate);
        // Set the flag from the RETENTION pass, which starts only AFTER this
        // callback returned: it means the worker resumed past the park.
        let resumed = Arc::clone(&resumed);
        let continuation = arm_scheduled_retention_pause(move || {
            resumed.store(true, Ordering::SeqCst);
        });
        std::mem::forget(continuation);
        None
    });
    spawn(schedule, registry, owner, shutdown)
}

#[test]
fn production_spawn_joins_the_parked_scheduler_and_binds_the_root_token() {
    on_big_stack(|| async {
        timeout(Duration::from_secs(170), run_production_spawn())
            .await
            .expect("production scheduled-worker acceptance timed out");
    });
}

async fn run_production_spawn() {
    let guard = FlakeGuard::with_watchdog(PRODUCTION_SPAWN_TEST, Duration::from_secs(150));
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    let backups = temp.path().join("backups");
    create_private_dir(&data_dir).expect("private data directory");
    create_private_dir(&backups).expect("private backup directory");

    let bridge = Arc::new(ProductionSpawnBridge {
        parked: Arc::new(Semaphore::new(0)),
        gate: Arc::new((Mutex::new(WorkerGate::default()), Condvar::new())),
        resumed: Arc::new(AtomicBool::new(false)),
        registry: Mutex::new(None),
    });
    *PRODUCTION_SPAWN_BRIDGE
        .lock()
        .expect("production spawn bridge") = Some(Arc::clone(&bridge));

    let handle = ServerLauncher::new(scheduled_config(&data_dir, &backups, "1h", true, 2))
        .with_bootstrap(BootstrapMode::Skip)
        .launch_with_backup_test(arm_then_spawn_production_worker)
        .await
        .expect("launch a real server through the production scheduled-worker spawn");
    guard.mark("launched through the production spawn");

    // `run_on_start` starts the run by itself; it parks in its post-run
    // callback, before its completion signal, and awaiting keeps the runtime driving.
    let permit = timeout(BOUND, bridge.parked.acquire())
        .await
        .expect("the production scheduled run must reach the parked handshake")
        .expect("parked semaphore open");
    drop(permit);
    guard.mark("production worker parked before its completion signal");

    let registry = bridge
        .registry
        .lock()
        .expect("registry slot")
        .clone()
        .expect("the launch callback recorded the launcher's registry");
    // The launcher's ROOT token, cloned only to know WHEN the root cancel happened.
    let root = handle.shutdown_token.clone();
    let mut shutdown = tokio::spawn(handle.shutdown());
    timeout(BOUND, root.cancelled())
        .await
        .expect("handle.shutdown() must cancel the launcher's root shutdown token");
    let parked_status = registry.status();
    guard.mark("root shutdown requested, worker still parked");

    // The run's `CancelFlag` derives from the ROOT token at admission, so the
    // root cancel flips it while the worker is parked. Mutant: a fresh token
    // instead of the launcher's root token leaves it clear.
    assert!(
        parked_status.running,
        "the parked run must still be registered: {parked_status:?}"
    );
    assert!(
        parked_status.cancel_requested,
        "the parked run's cancel flag must be the launcher's ROOT token, so the root \
         cancel sets it while the worker is still parked (mutant: a fresh token \
         instead of the root token leaves it clear): {parked_status:?}"
    );
    assert!(
        !bridge.resumed.load(Ordering::SeqCst),
        "the worker must still be parked after the root cancel"
    );

    let still_pending = timeout(PENDING_WINDOW, &mut shutdown).await.is_err();
    guard.mark("shutdown stayed pending past the pending window");

    release_worker(&bridge.gate);
    let report = timeout(BOUND, shutdown)
        .await
        .expect("shutdown must finish once the parked worker was released")
        .expect("shutdown task");
    guard.mark("worker released, shutdown drained");

    // Mutant: dropping `backup_schedule_task` instead of joining it lets
    // `handle.shutdown()` return while the run is still parked.
    assert!(
        still_pending,
        "handle.shutdown() must NOT return while the scheduled worker is parked \
         (mutant: the launcher stops awaiting the scheduled task)"
    );
    assert!(
        bridge.resumed.load(Ordering::SeqCst),
        "the released worker must resume and run its post-release cleanup"
    );
    assert!(
        report.is_clean(),
        "a shutdown that drained the released worker must be clean: {report:?}"
    );
}

// ---------------------------------------------------------------------------
// (a2) Production spawn path + a worker parked INSIDE RETENTION
// ---------------------------------------------------------------------------

const RETENTION_PARK_TEST: &str = "shutdown_waits_for_parked_retention_and_binds_the_root_token";

/// Separate bridge slot: never races the earlier production-spawn test's slot.
static RETENTION_PARK_BRIDGE: Mutex<Option<Arc<ProductionSpawnBridge>>> = Mutex::new(None);

/// The `launch_with_backup_test` spawn argument: record the launcher's registry,
/// arm the post-run callback that installs the retention pause ON THE RUNNER
/// THREAD, then call the PRODUCTION [`spawn`]. The pause fires at the start of
/// the retention pass (after the copy outcome) and parks the worker there.
fn arm_retention_park_then_spawn(
    schedule: ResolvedSchedule,
    registry: Arc<OnlineBackupRegistry>,
    owner: String,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let bridge = RETENTION_PARK_BRIDGE
        .lock()
        .expect("retention park bridge")
        .clone()
        .expect("the test installs the bridge before launching");
    *bridge.registry.lock().expect("registry slot") = Some(Arc::clone(&registry));
    let parked = Arc::clone(&bridge.parked);
    let gate = Arc::clone(&bridge.gate);
    registry.arm_post_run_callback_for_shutdown_test(move |_published| {
        let parked = Arc::clone(&parked);
        let gate = Arc::clone(&gate);
        let pause = arm_scheduled_retention_pause(move || {
            parked.add_permits(1);
            wait_for_release(&gate);
        });
        // One-shot: the pause removes itself from the thread-local when it
        // fires; the zero-sized guard would only clear an UNFIRED install.
        std::mem::forget(pause);
        None
    });
    spawn(schedule, registry, owner, shutdown)
}

#[test]
fn shutdown_waits_for_parked_retention_and_binds_the_root_token() {
    on_big_stack(|| async {
        timeout(Duration::from_secs(170), run_retention_park())
            .await
            .expect("parked-retention shutdown acceptance timed out");
    });
}

async fn run_retention_park() {
    let guard = FlakeGuard::with_watchdog(RETENTION_PARK_TEST, Duration::from_secs(150));
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    let backups = temp.path().join("backups");
    create_private_dir(&data_dir).expect("private data directory");
    create_private_dir(&backups).expect("private backup directory");

    let bridge = Arc::new(ProductionSpawnBridge {
        parked: Arc::new(Semaphore::new(0)),
        gate: Arc::new((Mutex::new(WorkerGate::default()), Condvar::new())),
        resumed: Arc::new(AtomicBool::new(false)),
        registry: Mutex::new(None),
    });
    *RETENTION_PARK_BRIDGE.lock().expect("retention park bridge") = Some(Arc::clone(&bridge));

    let handle = ServerLauncher::new(scheduled_config(&data_dir, &backups, "1h", true, 2))
        .with_bootstrap(BootstrapMode::Skip)
        .launch_with_backup_test(arm_retention_park_then_spawn)
        .await
        .expect("launch a real server through the production scheduled-worker spawn");
    guard.mark("launched through the production spawn");

    // The run starts by itself (`run_on_start`); retention then parks.
    let permit = timeout(BOUND, bridge.parked.acquire())
        .await
        .expect("the run must reach the retention park")
        .expect("parked semaphore open");
    drop(permit);
    guard.mark("production worker parked inside retention");

    // (1) The COPY outcome is already published while retention is parked.
    let parked_metrics = backup_metrics_snapshot();
    assert!(
        parked_metrics.last_success_timestamp_seconds > 0,
        "the successful scheduled copy must be published BEFORE retention: {parked_metrics:?}"
    );
    assert_eq!(
        parked_metrics.scheduled_failure_streak, 0,
        "a successful copy must keep the failure streak at zero while retention runs"
    );

    // (3a) The instance lock is still held while retention is parked.
    assert!(
        crate::maintenance_lock::acquire_server(&data_dir).is_err(),
        "the data-directory lock must still be held while retention is parked"
    );

    // (2) Shutdown must not return during the park (mutant: FINISHED sent early).
    let root = handle.shutdown_token.clone();
    let mut shutdown = tokio::spawn(handle.shutdown());
    timeout(BOUND, root.cancelled())
        .await
        .expect("handle.shutdown() must cancel the launcher's root token");
    let still_pending = timeout(PARKED_PROBE, &mut shutdown).await.is_err();
    guard.mark("shutdown stayed pending while retention was parked");

    release_worker(&bridge.gate);
    let report = timeout(BOUND, shutdown)
        .await
        .expect("shutdown must finish once the parked retention was released")
        .expect("shutdown task");
    guard.mark("retention released, shutdown drained");

    let lock_after = crate::maintenance_lock::acquire_server(&data_dir);
    assert!(
        still_pending,
        "handle.shutdown() must NOT return while the worker is parked inside retention \
         (mutant: FINISHED sent at protocol completion, or only the outcome awaited)"
    );
    assert!(
        report.is_clean(),
        "a shutdown that drained retention must be clean: {report:?}"
    );
    // (3b) The lock is free only after shutdown returned.
    assert!(
        lock_after.is_ok(),
        "the data-directory lock must be free once shutdown drained retention"
    );
}

// ---------------------------------------------------------------------------
// (b) The alert inputs, on the real `/metrics` route
// ---------------------------------------------------------------------------

#[test]
fn live_alert_inputs_track_the_scheduled_lifecycle_on_the_real_metrics_route() {
    on_big_stack(|| async {
        timeout(Duration::from_secs(170), run_alert_inputs())
            .await
            .expect("live alert-input acceptance timed out");
    });
}

async fn run_alert_inputs() {
    let guard = FlakeGuard::with_watchdog(ALERT_INPUTS_TEST, Duration::from_secs(150));
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
    // Cancels BEFORE the report, so the scheduler has already left its loop
    // when the terminal signal becomes observable.
    let stop_after_next = Arc::new(AtomicBool::new(false));
    let handle = ServerLauncher::new(scheduled_config(&data_dir, &backups, "1h", true, 2))
        .with_bootstrap(BootstrapMode::Skip)
        .launch_with_backup_test({
            let stop_after_next = Arc::clone(&stop_after_next);
            move |resolved, registry, owner, shutdown| {
                let scheduler_shutdown = shutdown.child_token();
                let stop_token = scheduler_shutdown.clone();
                spawn_for_test(
                    resolved,
                    registry,
                    owner,
                    scheduler_shutdown,
                    TICK,
                    move |ok| {
                        if ok && stop_after_next.load(Ordering::SeqCst) {
                            stop_token.cancel();
                        }
                        let _ = send.send(ok);
                    },
                )
            }
        })
        .await
        .expect("launch a real server with the scheduled worker");
    guard.mark("server launched with a millisecond tick");
    let addr = handle
        .observability
        .as_ref()
        .expect("observability")
        .bound_addr;

    // --- Phase 1: the first scheduled success stamps the last-success gauge.
    await_outcome(&mut receive, true, "the first scheduled success").await;
    guard.mark("first scheduled success reported");
    let first_route = scrape_metrics(addr).await;
    let first_stamp = sample(&first_route, "shamir_backup_last_success_timestamp_seconds");
    assert!(
        first_stamp > 0,
        "the route must render a real success stamp, not a fabricated zero \
         (mutant: `note_success` never publishing): {first_route}"
    );
    assert!(
        first_stamp > baseline.last_success_timestamp_seconds,
        "the last-success stamp must ADVANCE past the value observed before the run \
         (mutant: a gauge pinned to the baseline or always zero): first={first_stamp} \
         before={}",
        baseline.last_success_timestamp_seconds
    );
    assert_eq!(
        metric_type(&first_route, "shamir_backup_last_success_timestamp_seconds"),
        "gauge",
        "the last-success stamp must render as a gauge: {first_route}"
    );
    // No failure and no retention failure has happened yet.
    assert_eq!(
        sample(&first_route, "shamir_backup_scheduled_failure_streak"),
        0,
        "the failure streak starts at zero after a success: {first_route}"
    );
    assert_eq!(
        metric_type(&first_route, "shamir_backup_scheduled_failure_streak"),
        "gauge"
    );
    assert_eq!(
        sample(&first_route, "shamir_backup_retention_failed_total"),
        baseline.retention_failed_total,
        "the retention-failure counter cannot advance before a retention failure: {first_route}"
    );
    assert_eq!(
        metric_type(&first_route, "shamir_backup_retention_failed_total"),
        "counter"
    );

    // --- Phase 2: a FORCED scheduled failure: a top-level `manifest.json.tmp`
    // DIRECTORY is materialised into the snapshot, so the manifest write fails.
    let blocker = data_dir.join("manifest.json.tmp");
    fs::create_dir(&blocker).expect("manifest blocker directory");
    fs::write(blocker.join("blocker"), b"block").expect("manifest blocker");
    guard.mark("manifest blocker installed");
    // Two reported failures: after the second, no later completion can reset
    // the streak.
    await_outcome(&mut receive, false, "the first forced scheduled failure").await;
    await_outcome(&mut receive, false, "the second forced scheduled failure").await;
    guard.mark("forced scheduled failures reported");
    let failed_route = scrape_metrics(addr).await;
    let streak = sample(&failed_route, "shamir_backup_scheduled_failure_streak");
    assert!(
        streak > 0,
        "the failure streak must be nonzero after forced scheduled failures \
         (mutant: a failure that never reaches `note_scheduled`, or a gauge always \
         pinned to zero): {failed_route}"
    );
    assert_eq!(
        metric_type(&failed_route, "shamir_backup_scheduled_failure_streak"),
        "gauge"
    );

    // --- Phase 3: the repair, then the streak must fall back to zero.
    fs::remove_dir_all(&blocker).expect("manifest blocker removed");
    guard.mark("manifest blocker removed");
    await_outcome(&mut receive, true, "the scheduled success after the repair").await;
    guard.mark("scheduled success after the repair reported");
    let recovered_route = scrape_metrics(addr).await;
    assert_eq!(
        sample(&recovered_route, "shamir_backup_scheduled_failure_streak"),
        0,
        "the failure streak must return to 0 after a success (mutant: a streak that \
         never resets): {recovered_route}"
    );
    assert_eq!(
        metric_type(&recovered_route, "shamir_backup_scheduled_failure_streak"),
        "gauge"
    );
    let second_stamp = sample(
        &recovered_route,
        "shamir_backup_last_success_timestamp_seconds",
    );
    assert!(
        second_stamp >= first_stamp,
        "run N's last-success stamp must not regress below run N-1's \
         (self-observed values, no wall-clock oracle): first={first_stamp} \
         second={second_stamp}"
    );

    // --- Phase 4: a FORCED retention failure at the registry level: a
    // DIRECTORY at the lease-marker path, so provisioning fails while the copy
    // still succeeds (ONE retention failure).
    let marker = backups.join(SNAPSHOT_LEASE_FILE_NAME);
    let marker_meta = fs::symlink_metadata(&marker).expect("lease marker provisioned at boot");
    assert!(marker_meta.is_file(), "lease marker must be a regular file");
    obstruct_lease_marker(&mut receive, &backups, &marker).await;
    guard.mark("lease marker obstructed");
    // Drain already-delivered outcomes: every run ADMITTED from here on
    // reports a retention failure.
    while receive.try_recv().is_ok() {}
    let retention_before = backup_metrics_snapshot().retention_failed_total;
    // Three successes: a run is admitted only after the previous record went
    // idle, which happens AFTER its retention pass, so the third proves at
    // least one obstructed run's retention failure was published.
    await_outcome(&mut receive, true, "the first obstructed-marker success").await;
    await_outcome(&mut receive, true, "the second obstructed-marker success").await;
    await_outcome(&mut receive, true, "the third obstructed-marker success").await;
    guard.mark("obstructed-marker runs reported");
    let settled_retention = backup_metrics_snapshot().retention_failed_total;
    assert!(
        settled_retention > retention_before,
        "a marker-provisioning failure must be accounted as a retention failure \
         (mutant: `MarkerProvisioning::Failed` publishing no failure): \
         before={retention_before} after={settled_retention}"
    );
    let retention_route = scrape_metrics(addr).await;
    let retention_failed = sample(&retention_route, "shamir_backup_retention_failed_total");
    assert!(
        retention_failed > baseline.retention_failed_total,
        "the retention-failure counter must ADVANCE on the real route (mutant: a \
         counter that never advances): before={} route={retention_failed}",
        baseline.retention_failed_total
    );
    assert!(
        retention_failed >= settled_retention,
        "the route must render the process counter monotonically: \
         route={retention_failed} observed={settled_retention}"
    );
    assert_eq!(
        metric_type(&retention_route, "shamir_backup_retention_failed_total"),
        "counter"
    );

    // --- Phase 5: repair the marker, then stop the scheduler ON the final
    // success, so the frozen stamp can be compared to the route exactly. The
    // moved-aside markers stay as non-timestamp entries (never candidates).
    fs::remove_dir(&marker).expect("lease marker obstruction repaired");
    stop_after_next.store(true, Ordering::SeqCst);
    guard.mark("scheduler armed to stop on the next success");
    await_outcome(&mut receive, true, "the final scheduled success").await;
    guard.mark("final success reported, the scheduler stopped itself");
    let frozen = backup_metrics_snapshot().last_success_timestamp_seconds;
    let final_route = scrape_metrics(addr).await;
    assert_eq!(
        sample(&final_route, "shamir_backup_last_success_timestamp_seconds"),
        frozen,
        "the route must render exactly the success stamp the test observed in-process \
         (mutant: a route value not sourced from the runner's snapshot)"
    );
    assert!(
        frozen >= second_stamp,
        "the frozen stamp must not regress below the previously observed one: \
         previous={second_stamp} frozen={frozen}"
    );
    assert_eq!(
        metric_type(&final_route, "shamir_backup_last_success_timestamp_seconds"),
        "gauge"
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

/// One real `/metrics` scrape, bounded like every other wait in this file.
async fn scrape_metrics(addr: SocketAddr) -> String {
    timeout(BOUND, scrape_metrics_unbounded(addr))
        .await
        .unwrap_or_else(|_| panic!("the /metrics route must answer within {BOUND:?}"))
}

async fn scrape_metrics_unbounded(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect /metrics");
    stream
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write metrics request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read metrics response");
    let (head, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    assert!(
        head.starts_with("HTTP/1.1 200 "),
        "unexpected response: {head}"
    );
    body.to_owned()
}

fn sample(metrics: &str, series: &str) -> u64 {
    metrics
        .lines()
        .find_map(|line| line.strip_prefix(series)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("missing /metrics series: {series}"))
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("invalid /metrics value for {series}: {error}"))
}

fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| resume_unwind(panic));
}
