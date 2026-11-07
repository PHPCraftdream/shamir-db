//! TSet-of-maps insert and read over the real wire. Moved from
//! `shamir-client/src/tests/interner/set_insert_tests.rs`, whose
//! `with_client` fixture lived in the moved-out `interner_cache_tests`
//! module.

use std::path::PathBuf;
use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_types::mpack;
use shamir_types::types::common::TSet;
use shamir_types::types::value::{QueryValue, Value};
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{Client, ConnectOptions};
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
        kdf_defaults: fast_kdf(),
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

/// Boot a server + connect a client. The closure receives an owned client and
/// must return it (the fixture closes it). This sidesteps the HRTB dance that
/// a borrowed closure would require.
async fn with_client<F, Fut>(f: F)
where
    F: FnOnce(Client) -> Fut,
    Fut: std::future::Future<Output = Client>,
{
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();

    let launcher = ServerLauncher {
        node_mode: Default::default(),
        config: make_config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");

    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("connect");

    // Provision a db + repo + table so write/interner ops have a target.
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("ic", ddl::create_db("ic"));
    client
        .execute("default", mk_db.build())
        .await
        .expect("create db");

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client
        .execute("ic", mk_table.build())
        .await
        .expect("create table");

    let client = f(client).await;

    client.close().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_set_of_maps_insert_and_read() {
    with_client(|client: Client| async move {
        let mut set = TSet::<QueryValue>::default();
        set.insert(mpack!({ "cold_nested": 37 }));
        let mut batch = Batch::new();
        batch.insert(
            "inserted",
            insert("items").row(mpack!({ "group": @(Value::Set(set)) })),
        );
        client
            .execute_with_touch("ic", batch.build())
            .await
            .expect("insert set of maps");
        assert!(client.resolve_field("ic", "main", "cold_nested").is_some());
        let mut read = Batch::new();
        read.query("rows", Query::from("items"));
        let response = client
            .execute("ic", read.build())
            .await
            .expect("read inserted row");
        let row = response.results["rows"].records[0].as_value();
        let items = row.get("group").unwrap().as_array().unwrap();
        assert_eq!(items[0].get("cold_nested"), Some(&Value::Int(37)));
        client
    })
    .await;
}
