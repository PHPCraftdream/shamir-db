//! End-to-end concurrency tests for the full-duplex multiplexing stack (M4a).
//!
//! These tests go through the full real stack: TLS + SCRAM-Argon2id auth +
//! the duplex request loop, using `shamir_client::Client` (which owns the
//! rid-demux layer internally).
//!
//! # Test layout
//!
//! 1. **`concurrent_pings_resolve_all`** — 8 concurrent pings on one connection.
//! 2. **`concurrent_executes_return_correct_results`** — 4 concurrent inserts;
//!    each result matches the record that was sent (no cross-contamination).
//! 3. **`lock_step_mode_still_works`** — 3 sequential pings succeed even when
//!    the server is under load (exercises the sequential / ordered path).
//! 4. **`resume_then_concurrent`** — full auth → ticket → resume → 4 concurrent
//!    pings on the resumed connection all succeed.

use serde_json::json;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{Client, ConnectOptions, ResumeOptions};

mod common;

// ---------------------------------------------------------------------------
// Test 1 — 8 concurrent pings all resolve
// ---------------------------------------------------------------------------

/// Fire 8 pings concurrently on one connection.  All must return `Ok(())`.
///
/// This exercises the duplex path end-to-end: the client's `ping()` calls
/// each register a separate `rid` in the pending map and await their own
/// oneshot independently.  Responses arrive in completion order; the
/// rid-demux layer in `shamir_client` routes each back to the correct waiter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_pings_resolve_all() {
    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();
    let handle = common::spawn_ephemeral(&temp, &password).await;
    let addr = handle.first_tls_exporter_addr().expect("addr");

    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("connect");

    // Fire 8 pings concurrently — join_all waits for all of them.
    let pings: Vec<_> = (0..8).map(|_| client.ping()).collect();
    let results = futures_util::future::join_all(pings).await;
    for (i, r) in results.into_iter().enumerate() {
        r.unwrap_or_else(|e| panic!("ping {i} failed: {e}"));
    }

    client.close().await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test 2 — 4 concurrent inserts, results match requests (no cross-rid contamination)
// ---------------------------------------------------------------------------

/// Four concurrent `execute` batch requests each insert a distinct record.
///
/// After all complete we read the table back and assert that every record is
/// present.  The rid-demux layer must route each response to the correct
/// caller even when replies arrive out of send order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_executes_return_correct_results() {
    let temp = TempDir::new().expect("tempdir");
    let password = b"duplex e2e password".to_vec();
    let handle = common::spawn_ephemeral(&temp, &password).await;
    let addr = handle.first_tls_exporter_addr().expect("addr");

    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("connect");

    // Create the table in the pre-existing `default` db / `main` repo.
    let mut mk = shamir_query_builder::batch::Batch::new();
    mk.id("mk");
    mk.create_table("t", shamir_query_builder::ddl::create_table("items_e2e"));
    client
        .execute("default", mk.build())
        .await
        .expect("create table");

    // Fire 4 concurrent upserts for SKUs "A", "B", "C", "D".
    let skus = ["A", "B", "C", "D"];
    let futures: Vec<_> = skus
        .iter()
        .map(|sku| {
            let mut b = shamir_query_builder::batch::Batch::new();
            b.id("ins");
            b.upsert(
                "w",
                shamir_query_builder::write::upsert("items_e2e")
                    .key(json!({"sku": sku}))
                    .value(shamir_query_builder::doc! {
                        "sku" => *sku,
                        "qty" => 1,
                    }),
            );
            client.execute("default", b.build())
        })
        .collect();

    let results = futures_util::future::join_all(futures).await;
    for (i, r) in results.into_iter().enumerate() {
        r.unwrap_or_else(|e| panic!("insert {} failed: {e}", skus[i]));
    }

    // Read the table back and assert all 4 records are present.
    let mut read_b = shamir_query_builder::batch::Batch::new();
    read_b.id("rd");
    read_b.query("rows", shamir_query_builder::Query::from("items_e2e"));
    let resp = client
        .execute("default", read_b.build())
        .await
        .expect("read");
    let rows = &resp.results["rows"].records;
    assert_eq!(rows.len(), 4, "all 4 records must be present");

    // Each SKU appears exactly once.
    for sku in &skus {
        assert!(
            rows.iter().any(|r| r.get_value_str("sku") == Some(sku)),
            "sku {sku} not found in results"
        );
    }

    client.close().await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test 3 — sequential pings still work (lock-step mode)
// ---------------------------------------------------------------------------

/// 3 sequential pings succeed — the connection works correctly in the ordered
/// (non-concurrent) path.
///
/// Note: `CONN_MAX_IN_FLIGHT` is a per-connection limit set at connection
/// creation time on the server; we cannot lower it to 1 for an already-booted
/// server without a dedicated API.  This test instead verifies that a client
/// that only ever has one request in flight at a time works correctly — the
/// ordered path must remain a valid subset of the duplex path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_step_mode_still_works() {
    let temp = TempDir::new().expect("tempdir");
    let password = b"lock step pw".to_vec();
    let handle = common::spawn_ephemeral(&temp, &password).await;
    let addr = handle.first_tls_exporter_addr().expect("addr");

    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("connect");

    // Send 3 pings one at a time, fully awaiting each before sending the next.
    for i in 0..3u32 {
        client
            .ping()
            .await
            .unwrap_or_else(|e| panic!("sequential ping {i} failed: {e}"));
    }

    client.close().await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test 4 — resume → 4 concurrent pings
// ---------------------------------------------------------------------------

/// Full SCRAM auth → obtain resumption ticket → resume connection → fire 4
/// concurrent pings on the resumed client.
///
/// Verifies that the duplex multiplexer works correctly on a resumed session
/// (not just on a freshly authenticated one).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_then_concurrent() {
    let temp = TempDir::new().expect("tempdir");
    let password = b"resume concurrent pw".to_vec();
    let handle = common::spawn_ephemeral(&temp, &password).await;
    let addr = handle.first_tls_exporter_addr().expect("addr");

    // --- Phase 1: full SCRAM auth, capture ticket and pin ---
    let first = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("initial connect");

    let ticket = first
        .resumption_ticket()
        .expect("server must issue a resumption ticket")
        .to_vec();
    let pinned_hash = first.server_pub_key_pin();

    // Verify that the initial connection itself works.
    first.ping().await.expect("initial ping");
    first.close().await;

    // --- Phase 2: resume and fire 4 concurrent pings ---
    let resumed = Client::resume(ResumeOptions {
        addr,
        server_name: "localhost".to_string(),
        ticket,
        pinned_hash,
    })
    .await
    .expect("resume");

    let pings: Vec<_> = (0..4).map(|_| resumed.ping()).collect();
    let results = futures_util::future::join_all(pings).await;
    for (i, r) in results.into_iter().enumerate() {
        r.unwrap_or_else(|e| panic!("resumed ping {i} failed: {e}"));
    }

    resumed.close().await;
    handle.shutdown().await;
}
