//! Quickstart (guide floor 0) e2e: the dead-simple KV path must work.
//!
//! Boots a server, connects as the bootstrap admin via the high-level
//! `Client`, creates a table in the pre-existing `default`/`main` store,
//! PUTs a document via `set`, GETs it via `from`. This is the exact path
//! documented in `docs/guide/00-quickstart.md` — keep them in sync.

use serde_json::json;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{Client, ConnectOptions};

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quickstart_kv_in_default_store() {
    let temp = TempDir::new().expect("tempdir");
    let admin_pw = b"change-me-admin".to_vec();
    let handle = common::spawn_ephemeral(&temp, &admin_pw).await;
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
    let mut mk_batch = shamir_query_builder::batch::Batch::new();
    mk_batch.id("mk");
    mk_batch.create_table("t", shamir_query_builder::ddl::create_table("kv"));
    let resp = client
        .execute("default", mk_batch.build())
        .await
        .expect("create_table");
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
