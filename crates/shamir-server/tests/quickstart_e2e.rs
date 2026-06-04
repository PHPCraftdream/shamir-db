//! Quickstart (guide floor 0) e2e: the dead-simple KV path must work.
//!
//! Boots a server, connects as the bootstrap admin via the high-level
//! `Client`, creates a table in the pre-existing `default`/`main` store,
//! PUTs a document via `set`, GETs it via `from`. This is the exact path
//! documented in `docs/guide/00-quickstart.md` — keep them in sync.

use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{BatchRequest, Client, ConnectOptions};
use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

fn make_test_config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        data_dir: data_dir.clone(),
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
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quickstart_kv_in_default_store() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"change-me-admin".to_vec();
    let launcher = ServerLauncher {
        config,
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(admin_pw.clone()),
        },
    };
    let handle = launcher.launch().await.expect("boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");

    // Step 2 — connect as the bootstrap admin.
    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(admin_pw),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("connect");

    // Step 3 — create a table in the pre-existing default/main store.
    let mk: BatchRequest = serde_json::from_value(json!({
        "id": "mk",
        "queries": {
            "t": { "create_table": "kv", "repo": "main" }
        }
    }))
    .unwrap();
    let resp = client.execute("default", mk).await.expect("create_table");
    assert!(resp.results.contains_key("t"), "create_table ok");

    // Step 4a — PUT.
    let mut put_b = shamir_query_builder::batch::Batch::new();
    put_b.id("put");
    put_b.upsert(
        "p",
        shamir_query_builder::write::upsert("kv")
            .key(json!({"id": "user:42"}))
            .value(shamir_query_builder::doc! {
                "id" => "user:42",
                "name" => "Alice",
                "score" => 7,
            }),
    );
    client.execute("default", put_b.build()).await.expect("put");

    // Step 4b — GET by key.
    let mut get_b = shamir_query_builder::batch::Batch::new();
    get_b.id("get");
    get_b.query(
        "g",
        shamir_query_builder::Query::from("kv").where_eq("id", "user:42"),
    );
    let resp = client.execute("default", get_b.build()).await.expect("get");
    let rows = &resp.results["g"].records;
    assert_eq!(rows.len(), 1, "one row for user:42");
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["score"], 7);

    handle.shutdown().await;
}
