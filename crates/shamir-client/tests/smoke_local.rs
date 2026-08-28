//! End-to-end smoke test for the SDK's local-IPC transport
//! ([`Client::connect_local`]).
//!
//! Mirrors `smoke.rs` (the TLS+TCP SDK smoke test) but boots a
//! `kind: unix` listener and connects through `Client::connect_local` —
//! Unix domain socket on POSIX, Windows Named Pipe on Windows, same test
//! body either way. This is the actual client-library e2e coverage the
//! wire-level `shamir-server` tests (`plain_tcp_e2e.rs`, `ipc_e2e.rs`)
//! don't provide: those hand-roll the SCRAM frames to prove the SERVER
//! accepts the transport; this one proves the SDK itself can drive it.

use std::path::PathBuf;
use std::time::Duration;

use shamir_types::mpack;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::write::upsert;
use shamir_client::builder::Query;
use shamir_client::{Client, ConnectLocalOptions};

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

/// Local-IPC listener address. On Unix, a socket path inside `temp`. On
/// Windows, a Named Pipe name — unique per call via `temp`'s own
/// randomly-generated directory name (a pipe name has no filesystem
/// meaning, so `temp` is reused purely as a source of per-test
/// uniqueness).
fn ipc_addr(temp: &TempDir) -> String {
    #[cfg(unix)]
    {
        temp.path()
            .join("shamir-smoke.sock")
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(windows)]
    {
        let tag = temp
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("shamir-smoke");
        format!(r"\\.\pipe\{tag}")
    }
}

fn make_config(temp: &TempDir, addr: &str) -> Config {
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
            kind: ListenerKind::Unix,
            addr: addr.to_string(),
            profile: ProfileKind::Plain,
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
async fn sdk_full_lifecycle_over_local_ipc() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let addr = ipc_addr(&temp);
    let password = b"correct horse battery staple".to_vec();

    let launcher = ServerLauncher {
        config: make_config(&temp, &addr),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let bound_addr = handle
        .first_ipc_path()
        .expect("the single unix listener must be bound")
        .to_string();

    // ---- SDK in action, over local IPC ----
    let client = Client::connect_local(ConnectLocalOptions {
        addr: bound_addr,
        username: "admin".into(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("connect_local");

    // Pin should have been captured during the TOFU step — TOFU/Ed25519
    // identity pinning is layered on SCRAM, not on TLS, so it still
    // applies over a transport with no TLS at all.
    let pin = client.server_pub_key_pin();
    assert_ne!(pin, [0u8; 32], "TOFU pin captured");

    // Resumption ticket should have been issued by default, same as TCP.
    assert!(
        client.resumption_ticket().is_some(),
        "server should issue a resumption ticket over the unix transport"
    );

    // 1. ping
    client.ping().await.expect("ping");

    // 2. create db
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("mk", ddl::create_db("prod"));
    let resp = client
        .execute("default", mk_db.build())
        .await
        .expect("create db");
    assert!(resp.results.contains_key("mk"));

    // 3. create repo + table
    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    let resp = client
        .execute("prod", mk_table.build())
        .await
        .expect("create table");
    assert!(resp.results.contains_key("mr"));
    assert!(resp.results.contains_key("tb"));

    // 4. write + read in one batch
    let mut batch = Batch::new();
    batch.id("rw");
    batch
        .try_upsert(
            "ins",
            upsert("items")
                .key(mpack!({"sku": "X1"}))
                .value(doc! { "sku" => "X1", "qty" => 42 }),
        )
        .unwrap();
    batch.query("rd", Query::from("items"));
    let work = batch.build();
    let resp = client.execute("prod", work).await.expect("rw");
    let rd = resp.results.get("rd").expect("rd alias");
    assert_eq!(rd.records.len(), 1);
    assert_eq!(rd.records[0].get_value_str("sku"), Some("X1"));
    assert_eq!(rd.records[0].get_value_i64("qty"), Some(42));

    // 5. clean close
    client.close().await;

    // Give the server a beat to register the disconnect, then shutdown.
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}
