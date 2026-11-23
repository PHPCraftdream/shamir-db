//! Ordered-shutdown deadline vs. the SCHEDULED-BACKUP WORKER, plus the two
//! launch-rollback paths.
//!
//! `shutdown_deadline_does_not_abandon_the_scheduled_worker` parks the REAL
//! native backup runner inside its post-run callback — after the protocol
//! returned, before its completion signal — and then runs the embedding
//! shutdown (`runtime::drain_server`) with a SHORT deadline. The shutdown
//! future must stay pending until the worker is released, the report must be
//! unclean, and the single-instance data-directory lock must be held while the
//! worker runs and free afterwards.
//!
//! The two rollback tests make a bind fail (the listener path and the
//! observability path) and assert that the ORIGINAL boot error comes back, that
//! the shared audit deadline bounds the rollback, and that the instance lock is
//! released again.
//!
//! Nothing synchronises with a sleep: the worker rendezvous is a semaphore
//! signalled from the runner thread, the release is a condition variable, and
//! every await is bounded by [`BOUND`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use shamir_private_dir::create_private_dir;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};
use tokio::sync::Semaphore;
use tokio::time::timeout;

use crate::boot::config::diagnostics::backup_online::schedule::runner::spawn_for_test;
use crate::boot::config::HttpListenerSettings;
use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig, TlsConfig,
};
use crate::db_handler::NodeMode;
use crate::server::{BootstrapMode, ServerLauncher};

const TEST_NAME: &str = "shutdown_deadline_does_not_abandon_the_scheduled_worker";
const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../deploy/server.example.ktav"
));

/// One generous bound for every wait: the Windows CI runner is up to 10x
/// slower than a workstation.
const BOUND: Duration = Duration::from_secs(180);
/// The deadline under test, in REAL time: short enough to expire while the
/// worker is parked.
const TEST_DEADLINE: Duration = Duration::from_millis(250);
/// How long the embedding shutdown future must stay pending after the worker
/// parked — an order of magnitude more than the deadline, so a future that
/// returned at the deadline cannot hide behind timing noise.
const PENDING_WINDOW: Duration = Duration::from_secs(3);

#[test]
fn shutdown_deadline_does_not_abandon_the_scheduled_worker() {
    on_big_stack(|| async {
        timeout(BOUND, run_scheduled_worker())
            .await
            .expect("scheduled-worker deadline test bounded");
    });
}

async fn run_scheduled_worker() {
    let guard = FlakeGuard::with_watchdog(TEST_NAME, Duration::from_secs(300));
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    let backups = temp.path().join("backups");
    create_private_dir(&data_dir).expect("private data directory");
    create_private_dir(&backups).expect("private backup directory");

    // The worker parks inside its post-run callback; `parked` is how the test
    // learns the run is in flight (semaphore: no permit is lost), `gate` is how
    // it releases the run again.
    let parked = Arc::new(Semaphore::new(0));
    let gate = Arc::new((Mutex::new(WorkerState::default()), Condvar::new()));
    let completed = Arc::new(AtomicBool::new(false));

    let handle = ServerLauncher::new(backup_config(data_dir.clone(), backups))
        .with_bootstrap(BootstrapMode::Skip)
        .launch_with_backup_test({
            let parked = Arc::clone(&parked);
            let gate = Arc::clone(&gate);
            let completed = Arc::clone(&completed);
            move |resolved, registry, owner, shutdown| {
                registry.arm_post_run_callback_for_shutdown_test({
                    let parked = Arc::clone(&parked);
                    let gate = Arc::clone(&gate);
                    move |_published: &Path| {
                        parked.add_permits(1);
                        wait_until(&gate, "worker release", |state| state.released);
                        None
                    }
                });
                spawn_for_test(
                    resolved,
                    registry,
                    owner,
                    shutdown.child_token(),
                    Duration::from_secs(3600),
                    move |ok| {
                        if ok {
                            completed.store(true, Ordering::SeqCst);
                        }
                    },
                )
            }
        })
        .await
        .expect("launch real server with the scheduled backup worker");
    guard.mark("launched");

    // The run starts by itself (run_on_start) and parks before its completion
    // signal. Waiting for it is an await, so the runtime keeps driving the
    // scheduler task that starts it.
    let parked_permit = timeout(BOUND, parked.acquire())
        .await
        .expect("the scheduled run must reach its post-run callback")
        .expect("parked semaphore open");
    drop(parked_permit);
    guard.mark("native runner parked before its completion signal");

    // Only now does the shutdown start: the deadline cannot pre-empt the run
    // start, only the wait for it.
    let mut shutdown = tokio::spawn(crate::runtime::drain_server(handle, TEST_DEADLINE));
    let still_pending = timeout(PENDING_WINDOW, &mut shutdown).await.is_err();
    // Still while the worker runs: the instance lock must be held.
    let lock_while_running = crate::maintenance_lock::acquire_server(&data_dir).is_err();
    guard.mark("deadline elapsed with the worker still parked");

    // Unconditional release, then join; assertions run only afterwards.
    release(&gate);
    let shutdown = timeout(BOUND, shutdown)
        .await
        .expect("the shutdown future must finish once the worker was released")
        .expect("shutdown task");
    let lock_after = crate::maintenance_lock::acquire_server(&data_dir);
    guard.mark("worker released, shutdown drained, lock released");

    assert!(
        still_pending,
        "the embedding shutdown future must stay pending while the scheduled worker runs"
    );
    assert!(
        lock_while_running,
        "the data-directory lock must still be held while the scheduled worker runs"
    );
    assert!(
        shutdown
            .errors()
            .iter()
            .any(|error| error.contains("graceful shutdown exceeded deadline")),
        "the deadline must mark the report unclean: {shutdown:?}"
    );
    assert_eq!(shutdown.exit_code(), 70, "unclean report: {shutdown:?}");
    assert!(
        completed.load(Ordering::SeqCst),
        "the scheduled run must still complete after the release (the worker was drained, not abandoned)"
    );
    assert!(
        lock_after.is_ok(),
        "the data-directory lock must be free once the worker finished"
    );
}

/// The runner-side rendezvous state.
#[derive(Clone, Copy, Default)]
struct WorkerState {
    /// The worker may finish its run.
    released: bool,
}

fn wait_until(
    gate: &(Mutex<WorkerState>, Condvar),
    stage: &str,
    ready: impl Fn(WorkerState) -> bool,
) {
    let (lock, condvar) = gate;
    let deadline = Instant::now() + BOUND;
    let mut state = lock.lock().expect("worker state");
    while !ready(*state) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (next, timed_out) = condvar
            .wait_timeout(state, remaining)
            .expect("worker state wait");
        state = next;
        if timed_out.timed_out() && !ready(*state) {
            panic!("{stage} did not happen within {BOUND:?}");
        }
    }
}

fn release(gate: &(Mutex<WorkerState>, Condvar)) {
    let (lock, condvar) = gate;
    let mut state = lock.lock().expect("worker state");
    state.released = true;
    condvar.notify_all();
}

#[test]
fn listener_bind_rollback_keeps_the_boot_error_and_releases_the_lock() {
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    create_private_dir(&data_dir).expect("private data directory");
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let occupied = held.local_addr().expect("held address").to_string();
    let config = bind_config(data_dir.clone(), occupied, "127.0.0.1:0".to_owned());

    let error = run_blocking(move || match block_on_launch(config) {
        Ok(_) => panic!("the occupied listener port must fail the bind"),
        Err(error) => error,
    });
    let message = error.to_string();
    assert!(
        message.contains("listener bind"),
        "the original boot error must be returned: {message}"
    );
    // The rollback ran to completion: nothing of this launch holds the instance
    // lock any more, so a retry after freeing the port is possible.
    let lock = crate::maintenance_lock::acquire_server(&data_dir);
    assert!(
        lock.is_ok(),
        "the launch rollback must release the data-directory lock"
    );
}

#[test]
fn observability_bind_rollback_keeps_the_boot_error_and_releases_the_lock() {
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    create_private_dir(&data_dir).expect("private data directory");
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let occupied = held.local_addr().expect("held address").to_string();
    let config = bind_config(data_dir.clone(), "127.0.0.1:0".to_owned(), occupied);

    let error = run_blocking(move || match block_on_launch(config) {
        Ok(_) => panic!("the occupied observability port must fail the bind"),
        Err(error) => error,
    });
    let message = error.to_string();
    assert!(
        message.contains("observability"),
        "the original boot error must be returned: {message}"
    );
    let lock = crate::maintenance_lock::acquire_server(&data_dir);
    assert!(
        lock.is_ok(),
        "the launch rollback must release the data-directory lock"
    );
}

/// Run one real launch on a roomy thread with its own current-thread runtime,
/// bounded so a rollback that never finishes fails instead of hanging CI.
fn block_on_launch(
    config: Config,
) -> Result<crate::server::ServerHandle, crate::server::BootError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    // The timer is built INSIDE the runtime: `timeout(..)` as a `block_on`
    // argument would construct it before the runtime is entered and panic.
    let launched = runtime.block_on(async move {
        timeout(
            BOUND,
            Box::pin(
                ServerLauncher {
                    config,
                    bootstrap: BootstrapMode::Skip,
                    node_mode: NodeMode::ReadWrite,
                }
                .launch(),
            ),
        )
        .await
    });
    match launched {
        Ok(result) => result,
        Err(_) => panic!("launch rollback did not finish within {BOUND:?}"),
    }
}

fn run_blocking<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    run_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(body())
    })
}

/// A real server whose only listener is a plain loopback HTTP listener and
/// whose scheduled backup runs once at start.
fn backup_config(data_dir: PathBuf, backups: PathBuf) -> Config {
    let text = format!(
        "{EXAMPLE}\nbackup: {{\ndestination: backups\ninterval: 1h\nrun_on_start: true\n}}\n"
    );
    let mut config: Config = ktav::from_str(&text).expect("server KTAV with a backup schedule");
    config.backup.as_mut().expect("backup schedule").destination = backups;
    config.argon2_concurrent_max = 64;
    finish_config(
        config,
        data_dir,
        "127.0.0.1:0".to_owned(),
        "127.0.0.1:0".to_owned(),
    )
}

/// A server config with the listener and observability addresses under the
/// test's control (both are bound by the launcher, in that order).
fn bind_config(data_dir: PathBuf, listener: String, observability: String) -> Config {
    let mut config = Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        tokens: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 64,
        listeners: Vec::new(),
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: crate::config::SecurityConfig::default(),
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: "127.0.0.1:0".into(),
            allow_public_metrics: false,
        },
        replication: None,
        backup: None,
    };
    config.listeners = vec![listener_config(listener)];
    finish_config(config, data_dir, "127.0.0.1:0".to_owned(), observability)
}

fn finish_config(
    mut config: Config,
    data_dir: PathBuf,
    listener: String,
    observability: String,
) -> Config {
    config.data_dir = data_dir.clone();
    if config.listeners.is_empty() {
        config.listeners = vec![listener_config(listener)];
    }
    config.tls = TlsConfig {
        cert_path: data_dir.join("cert.pem"),
        key_path: data_dir.join("key.pem"),
    };
    config.observability.addr = observability;
    config.observability.allow_public_metrics = false;
    config
}

fn listener_config(addr: String) -> ListenerConfig {
    ListenerConfig {
        kind: ListenerKind::Http,
        addr,
        profile: None,
        http: HttpListenerSettings {
            tls: Some(false),
            ..Default::default()
        },
        path: None,
        kdf_override: None,
        browser_origin_allowlist: Vec::new(),
    }
}
