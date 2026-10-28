//! Initial SCRAM identity rejection tests (RV-CLIENT-2D0142DD).

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::{Client, ConnectOptions};
use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

fn auth_test_config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
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
            Err(crate::ClientError::Handshake { ref source, .. })
                if source.as_ref().is_some_and(|err| {
                    err.to_string().contains("server_identity_changed")
                })
        ),
        "a substituted identity must fail with ServerIdentityChanged"
    );
    handle.shutdown().await;
}
