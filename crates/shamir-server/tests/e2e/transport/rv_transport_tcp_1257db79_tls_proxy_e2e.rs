//! TLS-terminating relay e2e (REAL production client): a man-in-the-middle
//! terminates TLS on both legs, but the application-layer SCRAM identity
//! proof is bound to each leg's TLS exporter — so a relayed proof must be
//! rejected at the origin, and the resumption ticket must never be disclosed
//! through the relay.
//! Byte-level ticket non-disclosure is NOT proven by this e2e — the wire-level
//! zero-byte oracle lives in shamir-client's resume_wire_tests.rs.

use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use zeroize::Zeroizing;

use shamir_client::{Client, ClientError, ConnectOptions, ResumeOptions};
use shamir_server::config::{
    Config, ConnectionSecurity, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig,
    ProfileKind, SecurityConfig, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use shamir_transport_tcp::tls::{
    generate_self_signed_server_cert, make_client_config_app_authenticated,
    make_server_config_from_pem, AppLayerServerAuth,
};

fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn relay_config(temp: &TempDir) -> Config {
    let data_dir = temp.path().to_path_buf();
    Config {
        durability: Default::default(),
        transactions: Default::default(),
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
                auth_init_timeout_ms: 5000,
                max_active_connections: 0, // unlimited for this test
                max_active_connections_per_ip: 0,
            },
            query_limits: Default::default(),
            tx: Default::default(),
            cursors: Default::default(),
            auth_init_rate_per_second: 1000,
            enable_experimental_migration_api: false,
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_transport_tcp_1257db79_tls_terminating_relay_rejects_relayed_proof() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().unwrap();
    let launcher = ServerLauncher {
        config: relay_config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(b"hunter2".to_vec()),
        },
    };
    let handle = launcher.launch().await.expect("launcher");
    let origin = handle.first_tls_exporter_addr().expect("bound");

    // ---- TLS-terminating relay in front of the origin. ----
    let (relay_cert, relay_key) =
        generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let relay_acceptor =
        TlsAcceptor::from(make_server_config_from_pem(&relay_cert, &relay_key).unwrap());
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();

    let relay_task = tokio::spawn(async move {
        let connector = TlsConnector::from(
            make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap(),
        );
        let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        for _ in 0..8 {
            let (inbound_tcp, _) = match relay_listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let client_side = match relay_acceptor.accept(inbound_tcp).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (mut client_read, mut client_write) = tokio::io::split(client_side);
            let origin_tcp = match TcpStream::connect(origin).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let origin_side = match connector.connect(server_name.clone(), origin_tcp).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (mut origin_read, mut origin_write) = tokio::io::split(origin_side);
            // client→origin: plain copy.
            let c2o = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut client_read, &mut origin_write).await;
                let _ = origin_write.shutdown().await;
            });
            // origin→client: plain copy.
            let o2c = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut origin_read, &mut client_write).await;
                let _ = client_write.shutdown().await;
            });
            let _ = c2o.await;
            let _ = o2c.await;
        }
    });

    // ---- CONTROL: direct connection to the origin must succeed. ----
    let client = tokio::time::timeout(
        Duration::from_secs(60),
        Client::connect(ConnectOptions {
            addr: origin,
            server_name: "localhost".into(),
            username: "admin".into(),
            password: Zeroizing::new(b"hunter2".to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(Duration::from_secs(5)),
            handshake_timeout: Some(Duration::from_secs(30)),
            request_timeout: Some(Duration::from_secs(10)),
        }),
    )
    .await
    .expect("control connect must not hang")
    .expect("direct connect to origin must succeed");
    let pin = client.server_pub_key_pin();
    let ticket = client
        .resumption_ticket()
        .map(|t| t.to_vec())
        .expect("ticket issued");
    drop(client);

    // ---- RELAYED CONNECT: the two TLS legs have different exporters, so the
    // exporter-bound SCRAM authentication message differs between client and
    // origin and the origin rejects the proof. This is an end-to-end check that
    // a relayed leg does not authenticate; it does NOT isolate a single
    // production line (the exporter is bound on both client and server sides,
    // and the client also verifies the server signature). The TLS
    // CertificateVerify callback is covered at verifier level by
    // src/tests/verifier_tests.rs, not here. The assertion excludes success and
    // timeout; it does not observe the origin's authentication decision itself.
    let relayed = tokio::time::timeout(
        Duration::from_secs(30),
        Client::connect(ConnectOptions {
            addr: relay_addr,
            server_name: "localhost".into(),
            username: "admin".into(),
            password: Zeroizing::new(b"hunter2".to_vec()),
            accept_new_host: true,
            trusted_pin: Some(pin),
            connect_timeout: Some(Duration::from_secs(5)),
            handshake_timeout: Some(Duration::from_secs(30)),
            request_timeout: Some(Duration::from_secs(10)),
        }),
    )
    .await
    .expect("relayed connect must not hang");
    let err = match relayed {
        Ok(_) => panic!("relayed connect must be rejected"),
        Err(e) => e,
    };
    assert!(
        !format!("{err:?}").contains("Timeout"),
        "relay rejection must be an AUTHENTICATION failure, not a timeout: {err:?}"
    );
    assert!(
        matches!(err, ClientError::Transport { .. } | ClientError::Handshake { .. }),
        "relayed connect must fail on the rejected SCRAM proof (Transport: origin rejects the proof and closes without auth_ok; Handshake: typed auth rejection), got {err:?}"
    );

    // ---- RELAYED RESUME: the client verifies the origin's exporter-bound
    // resume proof BEFORE writing the bearer ticket, so the resume fails on
    // the relayed leg — the control leg's ticket is not accepted through the
    // relay. BYTE-LEVEL ticket non-disclosure is NOT proven by this test (see
    // file header); the non-disclosure property is covered at wire level by
    // crates/shamir-client/src/tests/client/resume_wire_tests.rs (zero-byte
    // oracle).
    let resumed = tokio::time::timeout(
        Duration::from_secs(30),
        Client::resume(ResumeOptions {
            addr: relay_addr,
            server_name: "localhost".into(),
            ticket,
            pinned_hash: pin,
        }),
    )
    .await
    .expect("relayed resume must not hang");
    let resume_err = match resumed {
        Ok(_) => panic!("relayed resume must be rejected"),
        Err(e) => e,
    };
    assert!(
        !format!("{resume_err:?}").contains("Timeout"),
        "resume rejection must not be a timeout: {resume_err:?}"
    );

    relay_task.abort();
    handle.shutdown().await;
}
