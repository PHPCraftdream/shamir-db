use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::panic::resume_unwind;
use std::thread;
use std::time::Duration;

use shamir_private_dir::create_private_dir;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::boot::config::diagnostics::backup_online::metrics::backup_metrics_snapshot;
use crate::boot::config::diagnostics::backup_online::schedule::runner::spawn_for_test;
use crate::boot::config::HttpListenerSettings;
use crate::config::{Config, ListenerConfig, ListenerKind, TlsConfig};
use crate::server::{BootstrapMode, ServerLauncher};
use crate::sync::durability::backup::{verify_manifest, MANIFEST_FILE_NAME};

const TEST_NAME: &str = "scheduled_backups_verify_prune_and_export_metrics_on_a_real_server";
const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../deploy/server.example.ktav"
));

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

    let text = format!(
        "{EXAMPLE}\nbackup: {{\ndestination: backups\ninterval: 1m\nrun_on_start: true\nretention: {{\nkeep_last: 2\n}}\n}}\n"
    );
    let mut config: Config = ktav::from_str(&text).expect("server KTAV with backup schedule");
    config.backup.as_mut().expect("backup schedule").destination = backups.clone();
    config.tls = TlsConfig {
        cert_path: data_dir.join("cert.pem"),
        key_path: data_dir.join("key.pem"),
    };
    config.data_dir = data_dir;
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

    let baseline = backup_metrics_snapshot();
    let (send, mut receive) = mpsc::channel(3);
    let handle = ServerLauncher::new(config)
        .with_bootstrap(BootstrapMode::Skip)
        .launch_with_backup_test(|resolved, registry, owner, shutdown| {
            let scheduler_shutdown = shutdown.child_token();
            let mut successes = 0;
            spawn_for_test(
                resolved,
                registry,
                owner,
                scheduler_shutdown.clone(),
                Duration::from_secs(2),
                move |ok| {
                    if ok {
                        successes += 1;
                        if successes == 3 {
                            scheduler_shutdown.cancel();
                        }
                    }
                    let _ = send.try_send(ok);
                },
            )
        })
        .await
        .expect("launch real server with backup scheduler");
    guard.mark("server launched");

    for _ in 0..3 {
        assert_eq!(
            receive.recv().await,
            Some(true),
            "scheduled copy must succeed"
        );
        guard.mark("scheduled copy and retention finished");
    }
    let addr = handle
        .observability
        .as_ref()
        .expect("observability")
        .bound_addr;
    let metrics = scrape_metrics(addr).await;
    handle.shutdown().await;
    guard.mark("server stopped");

    let completed = sample(
        &metrics,
        "shamir_backup_scheduled_runs_total{result=\"completed\"}",
    );
    assert!(completed >= baseline.scheduled_runs_total[4] + 3);
    let deleted = sample(&metrics, "shamir_backup_retention_deleted_total");
    assert!(deleted > baseline.retention_deleted_total);
    assert_eq!(sample(&metrics, "shamir_backup_retention_untouched"), 2);
    assert!(sample(&metrics, "shamir_backup_last_success_timestamp_seconds") > 0);
    assert!(sample(&metrics, "shamir_backup_last_bytes") > 0);

    assert_eq!(
        fs::read(&foreign_payload).expect("foreign payload"),
        b"foreign payload"
    );
    assert_eq!(
        fs::read(&invalid_manifest).expect("invalid manifest"),
        b"invalid manifest"
    );
    let retained: Vec<_> = fs::read_dir(&backups)
        .expect("read backup root")
        .map(|entry| entry.expect("backup entry").path())
        .filter(|path| path != &foreign && path != &invalid)
        .collect();
    assert_eq!(retained.len(), 2, "unexpected retained paths: {retained:?}");
    for snapshot in retained {
        verify_manifest(&snapshot).expect("retained snapshot verifies");
    }
}

async fn scrape_metrics(addr: SocketAddr) -> String {
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
