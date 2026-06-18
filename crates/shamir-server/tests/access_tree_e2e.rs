//! End-to-end test for the `access_tree` DDL op over the wire.
//!
//! Boots a real server, authenticates as the bootstrap admin via the
//! high-level TLS+SCRAM [`Client`], requests the access tree, and asserts
//! the assembled shape comes back. This exercises the full online path the
//! `access-tree --connect` CLI uses: wire dispatch → admin gate (the admin
//! session maps to `Actor::System`, which passes `Manage` on the root) →
//! `ShamirDb::access_tree` assembly → `Client::execute` result extraction.

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{Client, ConnectOptions};
use shamir_server::access_tree::{fetch_tree, render, AccessTreeArgs};
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
async fn access_tree_over_the_wire_as_admin() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = match launcher.launch().await {
        Ok(h) => h,
        Err(e) => panic!("launcher boot failed: {e}"),
    };
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let client = Client::connect(ConnectOptions {
        addr: server_addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(admin_pw),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("admin connect");

    let mut tree_batch = shamir_query_builder::batch::Batch::new();
    tree_batch.id(1);
    tree_batch.access_tree("tree", shamir_query_builder::ddl::access_tree());
    let resp = client
        .execute("default", tree_batch.build())
        .await
        .expect("execute");
    let qr = resp.results.get("tree").expect("tree result present");
    let rec = qr.records.first().expect("one record");
    let rec_val = rec.as_value();
    let tree = rec_val
        .get("access_tree")
        .expect("access_tree field present");

    // Resource root assembled.
    assert_eq!(
        tree["resources"]["kind"].as_str(),
        Some("root"),
        "resources root present"
    );
    // The default db is durable, so it shows under the root.
    let dbs = tree["resources"]["children"].as_array().expect("db array");
    assert!(
        dbs.iter().any(|d| d["name"].as_str() == Some("default")),
        "default database present in the tree"
    );
    // Builtins always populate the functions list.
    let fns = tree["functions"].as_array().expect("functions array");
    assert!(
        fns.iter().any(|f| f["name"].as_str() == Some("argon2id")),
        "argon2id builtin present in functions"
    );
    // Principals section is always shaped (arrays present).
    assert!(tree["principals"]["users"].as_array().is_some());
    assert!(tree["principals"]["groups"].as_array().is_some());

    handle.shutdown().await;
}

/// Offline path: after the server has written its durable `data_dir` and
/// stopped, the `access-tree` command (no `--connect`) opens it directly
/// and renders the same tree. Exercises `fetch_tree` offline + `render`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn access_tree_offline_from_data_dir() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    // Boot once to materialise the durable system store (default db/repo),
    // then shut down so the redb single-writer lock is released.
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(b"admin-password".to_vec()),
    };
    let launcher = ServerLauncher {
        config: config.clone(),
        bootstrap,
    };
    let handle = launcher.launch().await.expect("boot");
    handle.shutdown().await;

    // Offline fetch — no --connect, runs as System against data_dir.
    let args = AccessTreeArgs::default();
    let tree = fetch_tree(&config, &args).await.expect("offline fetch");

    assert_eq!(tree["resources"]["kind"].as_str(), Some("root"));
    let dbs = tree["resources"]["children"].as_array().expect("dbs");
    assert!(
        dbs.iter().any(|d| d["name"].as_str() == Some("default")),
        "default db present offline"
    );

    // The renderer produces the human view without panicking and includes
    // the durable database.
    let text = render(&tree);
    assert!(
        text.contains("db default"),
        "rendered tree shows db default"
    );
}

/// Online mode validates required flags before opening a connection:
/// `--connect` without `--user` fails fast with a clear message (no socket
/// is touched, so this needs no running server).
#[tokio::test]
async fn access_tree_online_requires_user() {
    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let args = AccessTreeArgs {
        connect: Some("127.0.0.1:1".to_string()),
        user: None,
        ..Default::default()
    };
    let err = fetch_tree(&config, &args)
        .await
        .expect_err("online mode must require --user");
    assert!(
        err.to_string().contains("--user"),
        "error should name the missing --user flag, got: {err}"
    );
}
