//! Slow-loris defence test.
//!
//! Verifies the pre-handshake `auth_init_timeout`: a client that completes
//! the TLS handshake but never sends `auth_init` is dropped within the
//! configured timeout, freeing the per-connection task and buffers.
//!
//! Without the timeout this client would tie up server resources forever
//! and a few thousand of them would OOM the process.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_transport_tcp::tls::make_client_config_no_ca;

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
            },
            query_limits: Default::default(),
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn silent_client_after_tls_is_dropped_within_timeout() {
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

    // ----- "Silent" client: complete TLS, then never send. -----
    let connector = TlsConnector::from(make_client_config_no_ca());
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.expect("tcp connect");
    let tls = connector
        .connect(server_name, tcp)
        .await
        .expect("tls handshake");

    let started = Instant::now();
    // Try to read — the server should close the connection within the
    // timeout and we'll get EOF (Ok(0)) or a closed-pipe error.
    let mut tls = tls;
    let mut buf = [0u8; 1];
    let read_result = tokio::time::timeout(Duration::from_secs(2), tls.read(&mut buf)).await;
    let elapsed = started.elapsed();

    // The read should have completed (either EOF or error) — NOT timed out.
    let read = read_result.expect("server should drop us within ~300ms, not hang");
    match read {
        Ok(0) => {
            // Clean EOF — server closed.
        }
        Ok(_) => panic!("server sent unexpected bytes for a silent client"),
        Err(_e) => {
            // Pipe closed / reset — also acceptable.
        }
    }

    // Drop should be near the configured timeout (300ms), not instant
    // (would mean the handshake failed for a different reason) and not
    // multi-second (would mean the timeout didn't fire).
    assert!(
        elapsed >= Duration::from_millis(200),
        "dropped too fast ({:?}) — likely a different error path",
        elapsed
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "dropped too slow ({:?}) — timeout didn't fire",
        elapsed
    );

    handle.shutdown().await;
}
