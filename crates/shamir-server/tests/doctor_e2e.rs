//! Integration tests for the `doctor` CLI subcommand.
//!
//! Tests basic CLI functionality:
//! 1. Empty data dir → succeeds
//! 2. Data dir with table (via backup/restore) → succeeds
//! 3. Filter options are respected
//! 4. JSON output works

use std::path::Path;

use shamir_server::backup;
use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::doctor::DoctorArgs;
use shamir_server::server::{BootstrapMode, ServerLauncher};
use zeroize::Zeroizing;

fn make_test_config(data_dir: &Path) -> Config {
    Config {
        data_dir: data_dir.to_path_buf(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".to_string(),
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

/// Scenario 1: Empty data dir → succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_empty_data_dir_succeeds() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let config = make_test_config(temp.path());

    // Run doctor directly in-process.
    let args = DoctorArgs::default();
    let result = shamir_server::doctor::run(&config, &args).await;
    assert!(
        result.is_ok(),
        "doctor should succeed on empty data_dir: {:?}",
        result
    );
}

/// Helper: Boot a server, create a table, backup the data_dir.
async fn setup_data_with_table() -> (tempfile::TempDir, tempfile::TempDir) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = tempfile::TempDir::new().expect("tempdir");
    let config = make_test_config(temp.path());

    // Boot server and create a table.
    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw),
    };
    let launcher = ServerLauncher {
        config: config.clone(),
        bootstrap,
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let client = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: server_addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(b"admin-password".to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("admin connect");

    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id(1).create_table(
        "test_table",
        shamir_query_builder::ddl::create_table("test_table"),
    );
    client.execute("default", batch.build()).await.unwrap();
    drop(client);

    // Shutdown server.
    handle.shutdown().await;

    // Backup the data_dir to a new location (avoids file lock issues).
    let backup_dest = tempfile::TempDir::new().expect("backup tempdir");
    backup::backup(temp.path(), backup_dest.path()).expect("backup ok");

    (temp, backup_dest)
}

/// Scenario 2: Data dir with table (via backup) → succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_with_table_succeeds() {
    let (_original_temp, backup_temp) = setup_data_with_table().await;

    // Run doctor on the backup data_dir.
    let config = make_test_config(backup_temp.path());
    let args = DoctorArgs::default();
    let result = shamir_server::doctor::run(&config, &args).await;
    assert!(
        result.is_ok(),
        "doctor should succeed with table: {:?}",
        result
    );
}

/// Scenario 3: Filter options are respected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_filter_options_work() {
    let (_original_temp, backup_temp) = setup_data_with_table().await;

    // Run doctor with filters.
    let config = make_test_config(backup_temp.path());
    let args = DoctorArgs {
        db: Some("default".to_string()),
        repo: Some("default".to_string()),
        table: Some("test_table".to_string()),
        ..Default::default()
    };
    let result = shamir_server::doctor::run(&config, &args).await;
    assert!(
        result.is_ok(),
        "doctor with filters should succeed: {:?}",
        result
    );
}

/// Scenario 4: JSON output works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_json_output_works() {
    let (_original_temp, backup_temp) = setup_data_with_table().await;

    // Run doctor with --json.
    let config = make_test_config(backup_temp.path());
    let args = DoctorArgs {
        json: true,
        ..Default::default()
    };
    let result = shamir_server::doctor::run(&config, &args).await;
    assert!(result.is_ok(), "doctor --json should succeed: {:?}", result);
}
