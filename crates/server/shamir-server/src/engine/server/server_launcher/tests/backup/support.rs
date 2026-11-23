//! Shared fixtures of the real-server scheduled-backup acceptance tests.

use std::ffi::OsString;
use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::panic::resume_unwind;
use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::boot::config::diagnostics::backup_online::registry::arm_scheduled_retention_end;
use crate::boot::config::diagnostics::backup_online::OnlineBackupRegistry;
use crate::boot::config::HttpListenerSettings;
use crate::config::{Config, ListenerConfig, ListenerKind, TlsConfig};

/// Injected tick spacing: MILLISECONDS, so no assertion waits on a second.
pub(super) const TICK: Duration = Duration::from_millis(40);
/// Fixed base second of the injected snapshot-name clock: every run gets
/// `base + n`, so no two runs collide on the `<to>/<timestamp>` create.
pub(super) const SNAPSHOT_NAME_BASE: u64 = 1_700_000_000;
/// One generous bound for every wait (the Windows CI runner is up to 10x slower).
pub(super) const BOUND: Duration = Duration::from_secs(120);
/// Watchdog deadline: fires BEFORE nextest's 180 s per-test kill.
pub(super) const WATCHDOG: Duration = Duration::from_secs(120);
/// Outer bound around a test body: leaves the watchdog room to dump its timeline
/// first.
pub(super) const OUTER: Duration = Duration::from_secs(140);
/// How long the shutdown future is watched while the worker is parked: a
/// launcher that stopped awaiting the scheduled task returns within it.
pub(super) const PENDING_WINDOW: Duration = Duration::from_secs(5);
/// Short negative probe window for the retention park (see `run_retention_park`).
pub(super) const PARKED_PROBE: Duration = Duration::from_millis(500);

pub(super) const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../deploy/server.example.ktav"
));

/// A real scheduled run's outcome, as delivered by the runner's terminal
/// signal. `true` is `Completed`, `false` is `Failed`.
pub(super) type TerminalOutcome = bool;

/// Arm the NEXT run's runner-thread observer: its post-run callback runs on
/// that run's OWN runner thread, after the copy and before retention, so it
/// installs the retention-END seam there (the pause seam's counterpart) and
/// reports the published name. Re-armed before the next admission, which waits
/// for this run's `finished` (sent after retention).
pub(super) fn arm_run_observer(
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
pub(super) async fn await_outcome(
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
pub(super) fn metric_type<'a>(metrics: &'a str, series: &str) -> &'a str {
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
pub(super) async fn obstruct_lease_marker(
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
pub(super) fn scheduled_config(
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

/// The worker-side rendezvous state of the parked scheduled run.
#[derive(Clone, Copy, Default)]
pub(super) struct WorkerGate {
    released: bool,
}

/// Block the runner thread until the test releases the gate. Bounded: a test
/// that never releases fails instead of hanging the suite.
pub(super) fn wait_for_release(gate: &(Mutex<WorkerGate>, Condvar)) {
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

pub(super) fn release_worker(gate: &(Mutex<WorkerGate>, Condvar)) {
    let (lock, condvar) = gate;
    lock.lock().expect("worker gate").released = true;
    condvar.notify_all();
}

/// One real `/metrics` scrape, bounded like every other wait in this file.
pub(super) async fn scrape_metrics(addr: SocketAddr) -> String {
    timeout(BOUND, scrape_metrics_unbounded(addr))
        .await
        .unwrap_or_else(|_| panic!("the /metrics route must answer within {BOUND:?}"))
}

pub(super) async fn scrape_metrics_unbounded(addr: SocketAddr) -> String {
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

pub(super) fn sample(metrics: &str, series: &str) -> u64 {
    metrics
        .lines()
        .find_map(|line| line.strip_prefix(series)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("missing /metrics series: {series}"))
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("invalid /metrics value for {series}: {error}"))
}

pub(super) fn on_big_stack<F, Fut>(body: F)
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
