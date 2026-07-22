//! FG-5c — end-to-end tests for `Client::stream_cursor` over a real
//! `ServerLauncher` (TCP), a real `shamir_client::Client`, and a real SCRAM
//! handshake.
//!
//! Mirrors `tests/smoke.rs`'s boot/connect pattern verbatim. FG-5e (a
//! later, separate task) is where the full cross-SDK e2e matrix
//! (idle-timeout, per-session cap, cancel mid-stream, snapshot stability)
//! lives — this file's scope stays narrower: happy-path pagination across
//! several pages, error propagation (no panic), and an empty result set.
//! `CursorStream::close`'s "actually reaches the server" proof lives in
//! `crates/shamir-client/src/tests/cursor_stream_tests.rs` (a crate-internal
//! unit test) because verifying it needs `Client::roundtrip`
//! (`pub(crate)`), which an external integration test file cannot reach.

use std::path::PathBuf;

use futures::StreamExt;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::write::insert;
use shamir_client::builder::Query;
use shamir_client::{Client, ClientError, ConnectOptions};

use shamir_query_types::read::{At, Temporal};

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

/// Boot a real server + connect a real SDK client, both bound to a fresh
/// `TempDir`. Returns the launcher handle (for `shutdown()`), the
/// connected client, and the `TempDir` (kept alive for the test's duration).
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

/// Create `db`/`repo`/`table` and seed `n` rows (`{ sku: "kNNN", qty: NNN }`)
/// via one batch upsert per row.
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

// ---------------------------------------------------------------------------
// Happy path: N rows spanning 3+ pages at a small page_size, collected via
// `StreamExt::collect`, matches the seeded set in order (ORDER BY qty ASC).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_cursor_paginates_all_rows_in_order() {
    let (handle, client, _temp) = boot().await;

    // 7 rows / page_size 2 -> 4 pages (2,2,2,1).
    seed_rows(&client, 7).await;

    let query = Query::from("items").order_by_asc("qty").build();
    let stream = client.stream_cursor("prod", query, 2);
    let results: Vec<_> = stream.collect().await;

    assert_eq!(results.len(), 7, "must yield exactly the seeded row count");
    for (i, r) in results.iter().enumerate() {
        let rec = r.as_ref().unwrap_or_else(|e| panic!("row {i}: {e}"));
        assert_eq!(
            rec.get_value_i64("qty"),
            Some(i as i64),
            "row {i} out of order"
        );
    }

    client.close().await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Error propagation: an AsOf query is rejected per FG-5b's scope cut — the
// stream's first (and only) item must be Err(ClientError::Db{code,..}),
// not a panic.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_cursor_surfaces_asof_rejection_as_db_error() {
    let (handle, client, _temp) = boot().await;

    seed_rows(&client, 3).await;

    let mut query = Query::from("items").order_by_asc("qty").build();
    query.temporal = Temporal::AsOf { at: At::Version(1) };

    let mut stream = client.stream_cursor("prod", query, 2);
    let first = stream
        .next()
        .await
        .expect("stream must yield exactly one Err item");
    match first {
        Err(ClientError::Db { code, .. }) => {
            assert_eq!(code, "cursor_temporal_not_supported");
        }
        other => panic!("expected Err(ClientError::Db{{..}}), got {other:?}"),
    }

    // Stream must end cleanly after the error (no panic, no further items).
    assert!(stream.next().await.is_none());
    drop(stream);

    client.close().await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Empty result set: a query matching zero rows yields a stream that ends
// immediately with zero items (not an error, not a panic).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_cursor_empty_result_set_yields_no_items() {
    let (handle, client, _temp) = boot().await;

    seed_rows(&client, 3).await;

    let query = Query::from("items")
        .where_gt("qty", 1000_i64)
        .order_by_asc("qty")
        .build();

    let stream = client.stream_cursor("prod", query, 2);
    let results: Vec<_> = stream.collect().await;
    assert!(
        results.is_empty(),
        "no rows match -> zero items, not an error"
    );

    client.close().await;
    handle.shutdown().await;
}
