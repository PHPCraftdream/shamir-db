//! End-to-end smoke test for the SDK.
//!
//! Boots a real `ServerLauncher` against a TempDir, connects through the
//! SDK, exercises the canonical happy-path: ping, create_db, create_repo,
//! create_table, set, read, graceful close. Mirrors what
//! `crates/shamir-server/tests/mvp_e2e.rs` does in 436 lines, but in
//! ~80 because all the wire/SCRAM machinery is encapsulated.
//!
//! Verifies the SDK's API holds together against a real server running
//! the production code path.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{BatchRequest, Client, ConnectOptions};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
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

fn make_config(temp: &TempDir) -> Config {
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
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sdk_full_lifecycle() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();

    let launcher = ServerLauncher {
        config: make_config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");

    // ---- SDK in action ----
    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("connect");

    // Pin should have been captured during the TOFU step.
    let pin = client.server_pub_key_pin();
    assert_ne!(pin, [0u8; 32], "TOFU pin captured");

    // Resumption ticket should have been issued by default.
    assert!(
        client.resumption_ticket().is_some(),
        "server should issue a resumption ticket"
    );

    // 1. ping
    client.ping().await.expect("ping");

    // 2. create db
    let mk_db: BatchRequest = serde_json::from_value(json!({
        "id": "mk-db",
        "queries": { "mk": { "create_db": "prod" } }
    }))
    .expect("parse mk_db");
    let resp = client.execute("default", mk_db).await.expect("create db");
    assert!(resp.results.contains_key("mk"));

    // 3. create repo + table
    let mk_table: BatchRequest = serde_json::from_value(json!({
        "id": "mk-table",
        "queries": {
            "mr": { "create_repo": "main" },
            "tb": { "create_table": "items", "repo": "main" }
        }
    }))
    .expect("parse mk_table");
    let resp = client
        .execute("prod", mk_table)
        .await
        .expect("create table");
    assert!(resp.results.contains_key("mr"));
    assert!(resp.results.contains_key("tb"));

    // 4. write + read in one batch
    let work: BatchRequest = serde_json::from_value(json!({
        "id": "rw",
        "queries": {
            "ins": { "set": "items", "key": {"sku":"X1"}, "value": {"sku":"X1","qty":42} },
            "rd":  { "from": "items" }
        }
    }))
    .expect("parse work");
    let resp = client.execute("prod", work).await.expect("rw");
    let rd = resp.results.get("rd").expect("rd alias");
    assert_eq!(rd.records.len(), 1);
    assert_eq!(
        rd.records[0].get("sku").and_then(|v| v.as_str()),
        Some("X1")
    );
    assert_eq!(rd.records[0].get("qty").and_then(|v| v.as_i64()), Some(42));

    // 5. clean close
    client.close().await;

    // Give the server a beat to register the disconnect, then shutdown.
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}
