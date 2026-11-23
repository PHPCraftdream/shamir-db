//! Private support copies of the production-boot fixtures used by
//! `shapes_gauge_tests`: the same hard per-call bound, big-stack test
//! runner, launcher config, raw `/metrics` scrape and SCRAM cancel-client
//! setup as the gauge-binding fixture, kept local so the gauge-binding
//! test's own items stay private to their module.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use shamir_client::{Client, ConnectOptions};

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{self, RepoEngine};

use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig, TlsConfig,
};

/// Hard per-call bound: every await is bounded, never an unbounded
/// engine await.
pub(super) const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Existing launcher/boot test convention: deep boot and engine futures
/// need a roomy stack; one runner per test, no runtime-inside-runtime.
pub(super) fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

/// Full production-launcher config over a real temp data dir: one plain
/// loopback HTTP data listener (no TLS material needed), observability on
/// port 0 (the ACTUAL bound port is read through the test seam, never
/// picked or retried by this test), and a finite in-flight response-byte
/// budget the test can hold at full capacity.
pub(super) fn boot_config(data_dir: std::path::PathBuf) -> Config {
    Config {
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
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Http,
            addr: "127.0.0.1:0".into(),
            profile: None,
            http: crate::boot::config::HttpListenerSettings {
                tls: Some(false),
                ..Default::default()
            },
            path: None,
            kdf_override: None,
            browser_origin_allowlist: Vec::new(),
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: crate::config::SecurityConfig {
            query_limits: crate::config::QueryLimitsConfig {
                // Generous test-only allowance: the real wire-path seed
                // insert's serialized response is ~4.3 KiB, over the old
                // 4096 fixture limit; the budget cap below is >= this max
                // result size, so the hold-at-full-capacity logic is
                // unchanged.
                max_result_size_bytes: 16_384,
                max_inflight_response_bytes: Some(16_384),
                ..Default::default()
            },
            ..Default::default()
        },
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: "127.0.0.1:0".into(),
            allow_public_metrics: false,
        },
        replication: None,
        backup: None,
    }
}

/// Raw bounded HTTP GET over loopback (the observability server speaks
/// plain HTTP; `Connection: close` bounds the read). The response head is
/// checked for a real `HTTP/1.1 200` status line; only the body is
/// returned.
pub(super) async fn http_get(addr: SocketAddr, path: &str) -> String {
    let fut = async {
        let mut s = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect /metrics");
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.expect("write request");
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.expect("read response");
        let hdr_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(buf.len());
        let head = String::from_utf8_lossy(&buf[..hdr_end]).into_owned();
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "/metrics must answer HTTP 200, got head: {head:?}"
        );
        buf[hdr_end + 4..].to_vec()
    };
    tokio::time::timeout(CALL_TIMEOUT, fut)
        .await
        .expect("bounded metrics scrape")
        .iter()
        .map(|b| *b as char)
        .collect()
}

pub(super) const CANCEL_DB: &str = "cancel_app";
pub(super) const CANCEL_ADMIN: &str = "admin";
pub(super) const CANCEL_PASSWORD: &[u8] = b"correct horse battery staple";

pub(super) async fn connect_cancel_client(
    addr: SocketAddr,
    trusted_pin: Option<[u8; 32]>,
) -> Client {
    tokio::time::timeout(
        CALL_TIMEOUT,
        Box::pin(Client::connect(ConnectOptions {
            addr,
            server_name: "localhost".into(),
            username: CANCEL_ADMIN.into(),
            password: zeroize::Zeroizing::new(CANCEL_PASSWORD.to_vec()),
            // Match the online-backup fixture's loopback TOFU policy on
            // first connect; require the same server key on the second.
            accept_new_host: trusted_pin.is_none(),
            trusted_pin,
            connect_timeout: Some(CALL_TIMEOUT),
            handshake_timeout: Some(CALL_TIMEOUT),
            request_timeout: Some(CALL_TIMEOUT),
        })),
    )
    .await
    .expect("TLS + SCRAM connect deadline")
    .expect("TLS + SCRAM connect")
}

pub(super) async fn setup_cancel_table(client: &Client) {
    // Same typed persistent DDL as the online-backup fixture.
    let mut db = Batch::new();
    db.create_db(CANCEL_DB, ddl::create_db(CANCEL_DB));
    let mut repo = Batch::new();
    repo.create_repo("main", ddl::create_repo("main").engine(RepoEngine::Fjall));
    let mut table = Batch::new();
    table.create_table("items", ddl::create_table("items").repo("main"));
    for (db, batch) in [("default", db), (CANCEL_DB, repo), (CANCEL_DB, table)] {
        tokio::time::timeout(CALL_TIMEOUT, client.execute(db, batch.build()))
            .await
            .expect("persistent fixture DDL deadline")
            .expect("persistent fixture DDL");
    }
}
