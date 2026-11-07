//! Initial SCRAM identity rejection tests (RV-CLIENT-2D0142DD).

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{Client, ConnectOptions, ResumeOptions, CURRENT_QUERY_LANG_VERSION};
use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

fn auth_test_config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        tokens: Default::default(),
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
            profile: Some(ProfileKind::TlsExporter),
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
            http: Default::default(),
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_2d0142dd_trusted_pin_rejects_substituted_identity() {
    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();
    let launcher = ServerLauncher {
        node_mode: Default::default(),
        config: auth_test_config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");
    let result = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password),
        accept_new_host: false,
        trusted_pin: Some([0xA5; 32]),
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await;
    assert!(
        matches!(
            result,
            Err(shamir_client::ClientError::Handshake { ref source, .. })
                if source.as_ref().is_some_and(|err| {
                    err.to_string().contains("server_identity_changed")
                })
        ),
        "a substituted identity must fail with ServerIdentityChanged"
    );
    handle.shutdown().await;
}

fn config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        tokens: Default::default(),
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
            profile: Some(ProfileKind::TlsExporter),
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
            http: Default::default(),
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

async fn start() -> (
    TempDir,
    shamir_server::server::ServerHandle,
    std::net::SocketAddr,
) {
    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();
    let handle = ServerLauncher {
        node_mode: Default::default(),
        config: config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password),
        },
    }
    .launch()
    .await
    .expect("server launch");
    let addr = handle.first_tls_exporter_addr().expect("bound listener");
    (temp, handle, addr)
}

fn connect_options(addr: std::net::SocketAddr, pin: Option<[u8; 32]>) -> ConnectOptions {
    ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(b"correct horse battery staple".to_vec()),
        accept_new_host: pin.is_none(),
        trusted_pin: pin,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_2d08f353_public_resume_after_rejected_pin() {
    let (_temp, handle, addr) = start().await;
    let initial = Client::connect(connect_options(addr, None))
        .await
        .expect("initial connect");
    let ticket = initial
        .resumption_ticket()
        .expect("resumption ticket")
        .to_vec();
    let pin = initial.server_pub_key_pin();
    let session_id = initial.session_id();
    initial.close().await;

    let rejected = Client::resume(ResumeOptions {
        addr,
        server_name: "localhost".into(),
        ticket: ticket.clone(),
        pinned_hash: [0xA5; 32],
    })
    .await;
    assert!(matches!(
        rejected,
        Err(shamir_client::ClientError::Handshake { ref context, .. })
            if context.contains("identity does not match pinned hash")
    ));

    // Three connects right after boot exceed the warm-up per-subnet accept
    // budget; a rate-limited connection is dropped before the proof frame, so
    // the ticket is never disclosed and a retry is safe.
    let mut attempts = 0;
    let resumed = loop {
        attempts += 1;
        match Client::resume(ResumeOptions {
            addr,
            server_name: "localhost".into(),
            ticket: ticket.clone(),
            pinned_hash: pin,
        })
        .await
        {
            Err(shamir_client::ClientError::Transport { context, .. })
                if context == "read resume_proof_ok" && attempts < 20 =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            other => break other.expect("resume with trusted pin"),
        }
    };
    assert_ne!(resumed.session_id(), session_id);
    assert_eq!(resumed.server_pub_key_pin(), pin);
    assert_eq!(
        u32::from(resumed.server_query_version()),
        CURRENT_QUERY_LANG_VERSION
    );
    resumed.ping().await.expect("ping resumed client");
    resumed.close().await;
    handle.shutdown().await;
}
