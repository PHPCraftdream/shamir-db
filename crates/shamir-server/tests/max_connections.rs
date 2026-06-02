//! Global max-connections cap — verifies that the server refuses new
//! connections (TCP RST before TLS) when `max_active_connections` is hit.
//!
//! Cap = 2 → first two TCP+TLS handshakes succeed, the third one is
//! either reset by the server immediately or hangs and times out (the
//! exact behaviour depends on whether the server's accept loop has
//! polled the third connection yet — both outcomes are acceptable
//! evidence that the cap is enforced).

use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;
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

fn make_config(temp: &TempDir, max_conns: usize) -> Config {
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
                // Long enough that the first two clients HOLD their TLS
                // session (no slow-loris drop) while we attempt the third.
                auth_init_timeout_ms: 30_000,
                max_active_connections: max_conns,
            },
            query_limits: Default::default(),
            tx: Default::default(),
            auth_init_rate_per_second: 1000,
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cap_two_refuses_third_concurrent_client() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().unwrap();
    let cfg = make_config(&temp, 2);
    let launcher = ServerLauncher {
        config: cfg,
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(b"hunter2".to_vec()),
        },
    };
    let handle = launcher.launch().await.expect("launcher");
    let addr = handle.first_tls_exporter_addr().expect("bound");

    let connector = TlsConnector::from(make_client_config_no_ca());
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();

    // First two — successful TLS handshakes. We HOLD them open (don't
    // drop) so the server's slot count stays at 2.
    let s1 = {
        let tcp = TcpStream::connect(addr).await.expect("tcp 1");
        connector
            .connect(server_name.clone(), tcp)
            .await
            .expect("tls 1")
    };
    let s2 = {
        let tcp = TcpStream::connect(addr).await.expect("tcp 2");
        connector
            .connect(server_name.clone(), tcp)
            .await
            .expect("tls 2")
    };

    // Give the server a moment to register both into the limiter — the
    // accept loop runs `try_acquire` synchronously per accept, but the
    // increment is observable only after the spawn-await scheduling.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Third — the server should refuse via `try_acquire == None`. Since
    // the limiter check happens BEFORE TLS, the server kernel-closes the
    // socket immediately. Different OSes surface this differently:
    //
    //   * Linux/macOS: TLS handshake fails with "connection reset" /
    //     "early eof" / similar within a few ms.
    //   * Windows (including this CI host): the kernel may complete the
    //     TCP 3-way handshake before the userland accept-loop drops it,
    //     so `TcpStream::connect` succeeds; the TLS handshake then sees
    //     the abrupt close.
    //
    // Either way, the TLS handshake MUST NOT complete successfully.
    let attempt_name = server_name.clone();
    let attempt = async {
        let tcp = TcpStream::connect(addr).await?;
        connector.connect(attempt_name, tcp).await
    };
    let result = tokio::time::timeout(Duration::from_secs(2), attempt).await;
    match result {
        Ok(Ok(_tls_stream)) => {
            panic!(
                "server should NOT have completed TLS handshake — cap is 2 and we have 2 active"
            );
        }
        Ok(Err(_e)) => { /* TLS failed — expected */ }
        Err(_elapsed) => { /* TLS hung past timeout — also expected (kernel held SYN-RECEIVED) */
        }
    }

    // Sanity: drop one of the open sessions, give the server a moment,
    // a new client should be accepted again.
    drop(s1);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _s3 = {
        let tcp = TcpStream::connect(addr).await.expect("tcp 3 after release");
        let r = tokio::time::timeout(Duration::from_secs(3), connector.connect(server_name, tcp))
            .await
            .expect("tls 3 within timeout")
            .expect("tls 3 should succeed after slot frees");
        r
    };

    let _ = s2;
    handle.shutdown().await;
}
