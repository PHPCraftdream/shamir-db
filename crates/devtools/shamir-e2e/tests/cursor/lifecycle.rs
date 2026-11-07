//! FG-5e — the two cursor-contour gaps NOT already covered by FG-5a/b/c/d's
//! own test suites, observed through the real `shamir-client` SDK against a
//! real `ServerLauncher` (TCP):
//!
//! 1. **Idle-timeout eviction**, proven through `Client::stream_cursor` /
//!    `CursorStream` rather than the server-side registry/reaper directly
//!    (FG-5b already covers the registry level in
//!    `crates/server/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`).
//! 2. **Per-session open-cursor cap rejection**, proven through
//!    `Client::stream_cursor` rather than the registry's cap logic directly
//!    (again already covered at the registry level by FG-5b) — including
//!    that a second session (a second `Client::connect`) is unaffected by
//!    the first session's cap.
//!
//! Everything else in the "full cursor contour" (happy-path pagination,
//! cancel/close mid-stream reaching the server, AsOf/empty-result error
//! propagation, MVCC snapshot stability) is already proven by
//! `crates/client/shamir-client/tests/cursor_stream.rs` (FG-5c) and
//! `crates/client/shamir-client/src/tests/cursor_stream_tests.rs` — this file does
//! not duplicate any of that.
//!
//! The background reaper sweep interval (`DEFAULT_CURSOR_REAPER_INTERVAL`,
//! 5s) is hardcoded server-side (`server_launcher.rs`), not configurable —
//! only `idle_timeout_secs` is. So the idle-timeout test below sleeps past
//! BOTH the configured idle window AND a full reaper sweep, not just the
//! idle window alone.

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::write::insert;
use shamir_client::builder::Query;
use shamir_client::{Client, ClientError, ConnectOptions};

use shamir_server::config::{
    Config, CursorLimitsConfig, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig,
    ProfileKind, SecurityConfig, TlsConfig,
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

fn make_config(temp: &TempDir, cursors: CursorLimitsConfig) -> Config {
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
        security: SecurityConfig {
            cursors,
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

/// Boot a real server (with the given cursor limits) + connect a real SDK
/// client, both bound to a fresh `TempDir`. Returns the launcher handle (for
/// `shutdown()`), the connected client, and the `TempDir` (kept alive for
/// the test's duration).
async fn boot(cursors: CursorLimitsConfig) -> (ServerHandle, Client, TempDir) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();

    let launcher = ServerLauncher {
        node_mode: Default::default(),
        config: make_config(&temp, cursors),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");

    let client = connect_client(addr, &password).await;

    (handle, client, temp)
}

/// Connect an additional client (a second session) against an
/// already-booted server.
async fn connect_client(addr: std::net::SocketAddr, password: &[u8]) -> Client {
    Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("connect")
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
// Gap 1: idle-timeout eviction, observed through the real SDK.
//
// `idle_timeout_secs = 1`. Open a cursor, fetch the first page (so a
// `cursor_id` exists server-side), then do NOT poll it again for ~7 real
// seconds (1s idle window + the hardcoded 5s reaper sweep + slack). The
// next poll must surface `ClientError::Db{code: "cursor_expired", ..}`.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_cursor_idle_timeout_evicts_and_surfaces_cursor_expired() {
    let (handle, client, _temp) = boot(CursorLimitsConfig {
        max_cursors_per_session: 16,
        idle_timeout_secs: 1,
        max_cursor_page_size: 10_000,
    })
    .await;

    // 10 rows / page_size 2 -> 5 pages; stop after the first page so the
    // cursor is still open (has_more) when we let it idle out.
    seed_rows(&client, 10).await;

    let query = Query::from("items").order_by_asc("qty").build();
    let mut stream = client.stream_cursor("prod", query, 2);

    let first = stream
        .next()
        .await
        .expect("at least one row")
        .expect("first row must not error");
    assert_eq!(first.get_value_i64("qty"), Some(0));

    // Second item of the first page — still buffered client-side, no
    // network round-trip, so this does not reset the server-side idle
    // clock.
    let second = stream
        .next()
        .await
        .expect("second row")
        .expect("second row must not error");
    assert_eq!(second.get_value_i64("qty"), Some(1));

    // Idle window (1s) + one full reaper sweep (hardcoded 5s) + slack.
    tokio::time::sleep(Duration::from_secs(7)).await;

    // Next poll drains the buffer and issues FetchNext against the
    // now-reaped cursor.
    let after_idle = stream.next().await.expect("stream must yield an item");
    match after_idle {
        Err(ClientError::Db { code, .. }) => {
            assert_eq!(code, "cursor_expired");
        }
        other => panic!("expected Err(ClientError::Db{{code: cursor_expired, ..}}), got {other:?}"),
    }

    drop(stream);
    client.close().await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Gap 2: per-session open-cursor cap rejection, observed through the real
// SDK.
//
// `max_cursors_per_session = 2`. Open 2 cursors (poll each once so
// `CreateCursor` actually round-trips) — both succeed. A 3rd cursor's first
// polled item must be `Err(ClientError::Db{code: "cursor_limit_exceeded",
// ..})`. A second session (a second `Client::connect`) must be unaffected —
// it can still open its own cursor successfully, proving the cap is
// per-session, not global.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_cursor_per_session_cap_rejects_third_cursor_but_not_other_sessions() {
    let (handle, client, _temp) = boot(CursorLimitsConfig {
        max_cursors_per_session: 2,
        idle_timeout_secs: 60,
        max_cursor_page_size: 10_000,
    })
    .await;

    seed_rows(&client, 10).await;

    let query = || Query::from("items").order_by_asc("qty").build();

    // Cursor 1: open + poll once, keep alive (do not drop/close) so it
    // stays counted against the session's cap.
    let mut stream1 = client.stream_cursor("prod", query(), 2);
    let item1 = stream1
        .next()
        .await
        .expect("cursor 1 must yield an item")
        .expect("cursor 1 first item must not error");
    assert_eq!(item1.get_value_i64("qty"), Some(0));

    // Cursor 2: same.
    let mut stream2 = client.stream_cursor("prod", query(), 2);
    let item2 = stream2
        .next()
        .await
        .expect("cursor 2 must yield an item")
        .expect("cursor 2 first item must not error");
    assert_eq!(item2.get_value_i64("qty"), Some(0));

    // Cursor 3: over the cap of 2 -> first polled item must be the
    // cursor_limit_exceeded error.
    let mut stream3 = client.stream_cursor("prod", query(), 2);
    let item3 = stream3
        .next()
        .await
        .expect("cursor 3 must yield exactly one Err item");
    match item3 {
        Err(ClientError::Db { code, .. }) => {
            assert_eq!(code, "cursor_limit_exceeded");
        }
        other => panic!(
            "expected Err(ClientError::Db{{code: cursor_limit_exceeded, ..}}), got {other:?}"
        ),
    }

    // A different session (a second `Client::connect`) must be unaffected
    // by the first session's cap — it can open its own cursor successfully.
    let addr = handle.first_tls_exporter_addr().expect("bound");
    let other_client = connect_client(addr, b"correct horse battery staple").await;
    let mut other_stream = other_client.stream_cursor("prod", query(), 2);
    let other_item = other_stream
        .next()
        .await
        .expect("other session's cursor must yield an item")
        .expect("other session's first item must not error");
    assert_eq!(other_item.get_value_i64("qty"), Some(0));

    drop(stream1);
    drop(stream2);
    drop(stream3);
    drop(other_stream);
    other_client.close().await;
    client.close().await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Cursor release observed through the per-session cap (public API only).
//
// `max_cursors_per_session = 1`: while a cursor is registered, a second one
// is rejected with `cursor_limit_exceeded`; once the first is released the
// slot is free again. This is the public equivalent of the former
// crate-internal probe (a raw `FetchNext` against the same cursor id), which
// cannot distinguish "removed" from "owned by another session".
// ---------------------------------------------------------------------------

/// Poll `stream` once and return the first item's `qty`, or the error code.
async fn first_item_code_or_qty(
    stream: &mut shamir_client::CursorStream<'_>,
) -> Result<i64, String> {
    match stream.next().await.expect("stream yields an item") {
        Ok(row) => Ok(row.get_value_i64("qty").expect("qty")),
        Err(ClientError::Db { code, .. }) => Err(code),
        Err(other) => panic!("unexpected client error: {other:?}"),
    }
}

/// `close()` mid-stream must release the server-side cursor slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_mid_stream_releases_the_cursor_slot() {
    let (handle, client, _temp) = boot(CursorLimitsConfig {
        max_cursors_per_session: 1,
        idle_timeout_secs: 60,
        max_cursor_page_size: 10_000,
    })
    .await;

    // 10 rows / page_size 2 -> 5 pages: the cursor is still open server-side
    // after the first row.
    seed_rows(&client, 10).await;
    let query = || Query::from("items").order_by_asc("qty").build();

    let mut first = client.stream_cursor("prod", query(), 2);
    assert_eq!(first_item_code_or_qty(&mut first).await, Ok(0));

    // The slot is taken: a second cursor of the same session is rejected.
    let mut rejected = client.stream_cursor("prod", query(), 2);
    assert_eq!(
        first_item_code_or_qty(&mut rejected).await,
        Err("cursor_limit_exceeded".to_string()),
        "the open cursor must occupy the only slot"
    );
    drop(rejected);

    first.close().await.expect("close must reach the server");

    // After close() the slot is free again.
    let mut third = client.stream_cursor("prod", query(), 2);
    assert_eq!(
        first_item_code_or_qty(&mut third).await,
        Ok(0),
        "close() must have released the cursor slot server-side"
    );

    drop(third);
    client.close().await;
    handle.shutdown().await;
}

/// CR-C1 (R-11): a mid-pagination `FetchNext` failure (here: the table is
/// dropped between pages) must fire a best-effort `CancelCursor` before the
/// stream yields the error, so the cursor slot is not left pinned until the
/// idle-timeout reaper sweeps it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mid_pagination_error_best_effort_cancels_the_cursor() {
    let (handle, client, _temp) = boot(CursorLimitsConfig {
        max_cursors_per_session: 1,
        idle_timeout_secs: 60,
        max_cursor_page_size: 10_000,
    })
    .await;

    seed_rows(&client, 10).await;

    // A second, untouched table to probe the slot with afterwards.
    let mut mk_probe = Batch::new();
    mk_probe.id("mk-probe");
    mk_probe.create_table("tb", ddl::create_table("probe").repo("main"));
    client
        .execute("prod", mk_probe.build())
        .await
        .expect("create probe table");
    let mut seed_probe = Batch::new();
    seed_probe.insert(
        "p0",
        insert("probe").row(doc! { "sku" => "p000", "qty" => 0_i64 }),
    );
    client
        .execute("prod", seed_probe.build())
        .await
        .expect("seed probe row");

    let mut stream =
        client.stream_cursor("prod", Query::from("items").order_by_asc("qty").build(), 2);
    assert_eq!(first_item_code_or_qty(&mut stream).await, Ok(0));

    // Drop the table out from under the still-open cursor. `drop_table` is a
    // destructive op and needs a confirmation HMAC computed from the client's
    // own session key (same as `Client::create_scram_user`).
    let hmac_key = shamir_connect::common::crypto::derive_session_hmac_key(&client.session_id());
    let op =
        shamir_query_types::batch::BatchOp::DropTable(shamir_query_types::admin::DropTableOp {
            drop_table: "items".into(),
            repo: "main".into(),
            hmac: None,
            if_exists: false,
            cascade: false,
        });
    let canonical = shamir_query_types::hmac::canonical_batch_op("prod", &op)
        .expect("drop-table canonical form");
    let tag = shamir_query_types::hmac::compute_tag_hex(&hmac_key, &canonical);
    let mut drop_batch = Batch::new();
    drop_batch.drop_table(
        "dt",
        ddl::drop_table("items")
            .hmac(&tag)
            .build()
            .expect("drop-table op"),
    );
    client
        .execute("prod", drop_batch.build())
        .await
        .expect("drop table must succeed");

    // Row qty=1 is still buffered from the first page; the next poll issues the
    // FetchNext that fails against the dropped table.
    let second = stream.next().await.expect("second buffered row");
    second.expect("second row must not error (already fetched)");
    let err_item = stream.next().await.expect("stream yields the error item");
    assert!(
        err_item.is_err(),
        "FetchNext against a dropped table must surface as an Err item, got {err_item:?}"
    );
    assert!(stream.next().await.is_none());
    drop(stream);

    // The best-effort cancel ran before the error was yielded, so the only
    // slot is free: a cursor over the intact table opens.
    let mut probe =
        client.stream_cursor("prod", Query::from("probe").order_by_asc("qty").build(), 2);
    assert_eq!(
        first_item_code_or_qty(&mut probe).await,
        Ok(0),
        "the mid-pagination error must have released the cursor slot"
    );

    drop(probe);
    client.close().await;
    handle.shutdown().await;
}
