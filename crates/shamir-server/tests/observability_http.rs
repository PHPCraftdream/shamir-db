//! Observability HTTP server end-to-end test.
//!
//! Spawns the server with `observability.addr = 127.0.0.1:0`, then HTTP-GETs
//! every endpoint and verifies status codes + headline content.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig,
    ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_config(temp: &TempDir, obs_addr: &str) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
            profile: ProfileKind::TlsExporter,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: obs_addr.into(),
        },
        replication: None,
    }
}

/// Pick an OS-assigned free port by binding-then-closing — race-free
/// enough for tests on a quiet CI host.
async fn pick_free_port() -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

/// Raw HTTP GET returning status code and body bytes (needed for binary
/// responses such as the msgpack `/info` endpoint).
async fn http_get_raw(addr: SocketAddr, path: &str) -> (u16, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    // Crude HTTP parse: headers are always ASCII, so find "\r\n\r\n" in bytes.
    let hdr_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let hdr = String::from_utf8_lossy(&buf[..hdr_end]);
    let status: u16 = hdr
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = hdr_end + 4;
    let body = if body_start <= buf.len() {
        buf[body_start..].to_vec()
    } else {
        Vec::new()
    };
    (status, body)
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let (status, body) = http_get_raw(addr, path).await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn endpoints_return_expected_codes_and_content() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().unwrap();
    let obs_addr = pick_free_port().await;

    let cfg = make_config(&temp, &obs_addr.to_string());
    let launcher = ServerLauncher {
        config: cfg,
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(b"hunter2".to_vec()),
        },
    };
    let handle = launcher.launch().await.expect("launcher");

    // Give the HTTP server a moment to actually start serving.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // /healthz — always 200.
    let (status, body) = http_get(obs_addr, "/healthz").await;
    assert_eq!(status, 200, "healthz status");
    assert!(
        body.contains("ok"),
        "healthz body should say ok, got {:?}",
        body
    );

    // /readyz — should be 200 because the launcher marked ready before
    // returning.
    let (status, _body) = http_get(obs_addr, "/readyz").await;
    assert_eq!(status, 200, "readyz status after boot");

    // /metrics — Prometheus text. Should include the standard process_*
    // series from metrics-process, AND the application metrics
    // pre-registered by `observability::spawn` (so dashboards can
    // discover the names even before the first event).
    let (status, body) = http_get(obs_addr, "/metrics").await;
    assert_eq!(status, 200, "metrics status");
    assert!(
        body.contains("process_"),
        "metrics body must include process_* series, got first 200 bytes: {:?}",
        &body.chars().take(200).collect::<String>()
    );
    assert!(
        body.contains("auth_attempts_total"),
        "metrics body must include the application counter \
         `auth_attempts_total` (pre-registered via describe_counter!) \
         so Grafana picks it up before the first auth attempt; first \
         500 bytes: {:?}",
        &body.chars().take(500).collect::<String>()
    );

    // /info — msgpack-encoded server info.
    let (status, body) = http_get_raw(obs_addr, "/info").await;
    assert_eq!(status, 200, "info status");
    #[derive(serde::Deserialize)]
    struct InfoBody {
        uptime_seconds: u64,
        ready: bool,
    }
    let info: InfoBody = rmp_serde::from_slice(&body)
        .unwrap_or_else(|e| panic!("info body should decode as msgpack InfoBody: {e}"));
    assert!(info.ready, "info should say ready=true");
    // uptime_seconds is always present (just decoded it); sanity-check
    // it's a reasonable value (server just booted).
    assert!(info.uptime_seconds < 60, "uptime sanity");

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refuses_non_loopback_bind_without_opt_in() {
    // M-tier audit M5: spawning the observability HTTP server on a
    // non-loopback address without explicit `allow_public_metrics`
    // must fail before any port is bound. /metrics exposes lockout
    // counters that are useful signal for a distributed attacker.
    use shamir_server::observability::{spawn, ObservabilityError, ObservabilityState};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    let state = ObservabilityState::new();
    // Pick a non-loopback address — `0.0.0.0` is the obvious case but
    // it's also the canonical wildcard so a corresponding `bind` would
    // succeed if we let it. The guard must trip BEFORE the bind.
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
    let res = spawn(addr, state, false, None, false).await;
    match res {
        Err(ObservabilityError::NonLoopbackBindRejected(rejected)) => {
            assert_eq!(
                rejected.ip(),
                addr.ip(),
                "rejected addr must be the one we passed"
            );
        }
        other => panic!(
            "expected NonLoopbackBindRejected, got {:?}",
            other.as_ref().err()
        ),
    }
}
