//! `ServerHandle::shutdown()` contract: when it returns, every store of the
//! data dir is released, so the same dir can be relaunched at once.
//!
//! The relaunch below has NO retry on purpose: before the fix, shutdown
//! returned while detached tasks (buffered-store flushers, connection tasks)
//! still held the fjall handles, and the next boot failed with `Locked`.

use std::path::Path;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};
use zeroize::Zeroizing;

const CYCLES: usize = 8;
const PASSWORD: &[u8] = b"admin-password";

fn make_config(data_dir: &Path) -> Config {
    Config {
        transactions: Default::default(),
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
        // The pre-auth rate limit persists across restarts and runs at a quarter
        // of its rate for 60 s after each boot (spec §8.6); eight boots with one
        // login each would hit it, and a rate-limited connect looks like `early eof`.
        security: shamir_server::config::SecurityConfig {
            auth_init_rate_per_second: 100_000,
            ..Default::default()
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

async fn launch(data_dir: &Path) -> ServerHandle {
    ServerLauncher {
        config: make_config(data_dir),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(PASSWORD.to_vec()),
        },
    }
    .launch()
    .await
    .expect("launch must not hit a store lock left by the previous shutdown")
}

async fn connect(handle: &ServerHandle) -> shamir_client::Client {
    shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: handle.first_tls_exporter_addr().expect("bound address"),
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("admin connect")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relaunch_right_after_shutdown_never_hits_a_held_store() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let data_dir = temp.path().to_path_buf();

    for cycle in 0..CYCLES {
        let handle = launch(&data_dir).await;
        let client = connect(&handle).await;

        if cycle == 0 {
            let mut batch = shamir_query_builder::batch::Batch::new();
            batch
                .id(1)
                .create_table("items", shamir_query_builder::ddl::create_table("items"));
            client
                .execute("default", batch.build())
                .await
                .expect("create table");
        }

        // A buffered (write-behind) write right before shutdown: the
        // flusher of the buffered store is still alive when shutdown runs.
        let mut batch = shamir_query_builder::batch::Batch::new();
        batch.id("wr");
        batch.insert(
            "ins",
            shamir_query_builder::write::insert("items").rows([shamir_query_builder::doc! {
                "cycle" => cycle as i64
            }]),
        );
        client
            .execute("default", batch.build())
            .await
            .expect("insert");

        drop(client);
        let report = handle.shutdown().await;
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.exit_code(), 0);
    }

    let handle = launch(&data_dir).await;
    let client = connect(&handle).await;
    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id("rd");
    batch.query("rd", shamir_query_builder::Query::from("items"));
    let resp = client
        .execute("default", batch.build())
        .await
        .expect("read back");
    let rows = resp.results.get("rd").expect("rd alias").records.len();
    assert_eq!(
        rows, CYCLES,
        "every cycle's row must survive the relaunches"
    );

    drop(client);
    handle.shutdown().await;
}

/// Same contract with a client that is still connected when `shutdown()` is
/// called: the per-connection task (and the stores it reaches through its
/// handler context) must not outlive `shutdown()`, otherwise the relaunch hits
/// a held store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relaunch_right_after_shutdown_with_a_client_still_connected() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let data_dir = temp.path().to_path_buf();

    let handle = launch(&data_dir).await;
    let client = connect(&handle).await;
    let mut batch = shamir_query_builder::batch::Batch::new();
    batch
        .id(1)
        .create_table("items", shamir_query_builder::ddl::create_table("items"));
    client
        .execute("default", batch.build())
        .await
        .expect("create table");

    // The client stays connected across the shutdown.
    handle.shutdown().await;

    let handle = launch(&data_dir).await;
    drop(client);
    handle.shutdown().await;
}
