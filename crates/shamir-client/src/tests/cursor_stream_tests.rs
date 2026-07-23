//! FG-5c — crate-internal test proving `CursorStream::close()` actually
//! reaches the server.
//!
//! Lives here (rather than `crates/shamir-client/tests/cursor_stream.rs`)
//! because the proof needs `Client::roundtrip` — deliberately `pub(crate)`,
//! not `pub` — to drive a raw `FetchNext` against the SAME `cursor_id` the
//! stream opened, after `close()` returns, and assert the server reports
//! `cursor_not_found` (the registry entry is gone). An external integration
//! test in `tests/` cannot reach `pub(crate)` items — this file, compiled
//! as part of the `shamir-client` lib crate itself (`#[cfg(test)] mod
//! tests` in `lib.rs`), can.

use std::path::PathBuf;

use futures::StreamExt;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;

use shamir_query_types::wire::DbRequest;

use crate::{Client, ConnectOptions};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

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
            allow_public_metrics: false,
        },
        replication: None,
    }
}

async fn boot() -> (ServerHandle, Client, TempDir) {
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

    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("connect");

    (handle, client, temp)
}

async fn seed_rows(client: &Client, n: usize) {
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("mk", ddl::create_db("prod"));
    client
        .execute("default", mk_db.build())
        .await
        .expect("create db");

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client
        .execute("prod", mk_table.build())
        .await
        .expect("create table");

    let mut batch = Batch::new();
    for i in 0..n {
        batch.insert(
            format!("i{i}"),
            insert("items").row(doc! { "sku" => format!("k{i:03}"), "qty" => i as i64 }),
        );
    }
    client
        .execute("prod", batch.build())
        .await
        .expect("seed rows");
}

/// `close()` mid-stream: start iterating, stop partway, call `close()`, then
/// drive a raw `FetchNext` against the SAME `cursor_id` directly via
/// `Client::roundtrip` — proving `close()` actually reached the server (the
/// registry entry for this id is gone, so the server reports
/// `cursor_not_found`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_mid_stream_reaches_the_server() {
    let (handle, client, _temp) = boot().await;

    // 10 rows / page_size 2 -> 5 pages; stop after the first row so the
    // cursor is still very much open server-side when we close it.
    seed_rows(&client, 10).await;

    let query = Query::from("items").order_by_asc("qty").build();
    let mut stream = client.stream_cursor("prod", query, 2);

    let first = stream.next().await.expect("at least one row");
    let first = first.expect("first row must not error");
    assert_eq!(first.get_value_i64("qty"), Some(0));

    let cursor_id = stream
        .cursor_id()
        .expect("cursor_id must be known after at least one successful page");

    stream.close().await.expect("close must reach the server");

    // Drive a raw FetchNext against the same cursor_id — proves close()
    // actually removed it from the server's registry.
    let probe_req: DbRequest = crate::builder::cursor::fetch_next(cursor_id, Some(2));
    let resp = client.roundtrip(&probe_req).await;
    match resp {
        Err(crate::ClientError::Db { code, .. }) => {
            assert_eq!(
                code, "cursor_not_found",
                "cursor must be gone from the registry after close()"
            );
        }
        other => {
            panic!("expected Err(ClientError::Db{{code: cursor_not_found, ..}}), got {other:?}")
        }
    }

    client.close().await;
    handle.shutdown().await;
}
