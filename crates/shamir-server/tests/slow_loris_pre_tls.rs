//! Pre-handshake slow-loris defence test.
//!
//! Complements `slow_loris.rs` (which completes TLS then stalls on
//! `auth_init`). This file covers the EARLIER window identified in
//! audit §2a / top-5 #4: a client that opens a TCP connection but
//! never sends a TLS ClientHello at all.
//!
//! Before the fix, `acceptor.accept(tcp).await` had no timeout — the
//! spawned task (and its `ConnLimiter` slot) lived forever. Now the
//! accept loop wraps the TLS accept in `tokio::time::timeout(
//! ctx.auth_init_timeout, …)`, so the slot is released within roughly
//! `auth_init_timeout` of the TCP accept.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

use shamir_server::config::{
    Config, ConnectionSecurity, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig,
    ProfileKind, SecurityConfig, TlsConfig,
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

fn config_with_short_timeout(temp: &TempDir, timeout_ms: u64) -> Config {
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
        security: SecurityConfig {
            connection: ConnectionSecurity {
                auth_init_timeout_ms: timeout_ms,
                max_active_connections: 0, // unlimited for this test
                max_active_connections_per_ip: 0,
            },
            query_limits: Default::default(),
            tx: Default::default(),
            auth_init_rate_per_second: 1000,
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// A client that connects at the TCP layer but NEVER sends a TLS
/// ClientHello. The server must release the connection slot within
/// roughly `auth_init_timeout` — before the fix the slot leaked
/// indefinitely.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn silent_tcp_client_is_dropped_within_timeout() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().unwrap();
    // Aggressive 300ms timeout so the test is fast.
    let cfg = config_with_short_timeout(&temp, 300);
    let launcher = ServerLauncher {
        config: cfg,
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(b"hunter2".to_vec()),
        },
    };
    let handle = launcher.launch().await.expect("launcher");
    let addr = handle.first_tls_exporter_addr().expect("bound");

    // Connect at TCP layer only — deliberately do NOT perform a TLS
    // handshake. Hold the socket open so the server's acceptor is
    // waiting for bytes that never arrive.
    let mut tcp = TcpStream::connect(addr).await.expect("tcp connect");

    let started = Instant::now();
    // The server should close/reset the socket within ~auth_init_timeout.
    // We detect this by reading: EOF (Ok(0)) or a reset (Err) both prove
    // the server tore down its side. If the timeout is NOT applied, this
    // read hangs and the outer `tokio::time::timeout` fires — the test
    // then panics in the `expect` below.
    let mut buf = [0u8; 1];
    let read_result = tokio::time::timeout(Duration::from_secs(2), tcp.read(&mut buf)).await;
    let elapsed = started.elapsed();

    // Must NOT have timed out — the server must have closed us.
    let read = read_result.expect(
        "server should drop the silent TCP client within ~300ms, not let the read hang for 2s",
    );
    match read {
        Ok(0) => { /* clean EOF — server closed */ }
        Ok(_) => panic!("server sent unexpected bytes for a silent TCP client"),
        Err(_e) => { /* reset / pipe closed — also acceptable */ }
    }

    // The drop must be near the configured timeout (300ms), not instant
    // (would imply a different error path) and not multi-second (would
    // imply the timeout didn't fire — the bug we're guarding against).
    assert!(
        elapsed >= Duration::from_millis(200),
        "dropped too fast ({:?}) — likely a different error path",
        elapsed
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "dropped too slow ({:?}) — pre-handshake timeout didn't fire (the bug)",
        elapsed
    );

    // Clean up — ensure the socket is closed cleanly on our side too.
    let _ = tcp.shutdown().await;
    handle.shutdown().await;
}
