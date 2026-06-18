//! Integration tests for per-table buffer-config DDL via
//! `ShamirDb::execute`. All batch requests are built with
//! `shamir_query_builder` and round-tripped through MessagePack.

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::ddl::{BufConfig, BufPatch};

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

fn full_cfg() -> BufConfig {
    BufConfig {
        max_bytes: 1_048_576,
        max_entries: 500,
        ttl_ms: Some(7000),
        flush_interval_ms: 333,
        flush_batch_size: 48,
    }
}

#[tokio::test]
async fn get_buffer_config_returns_null_when_unset() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.get_buffer_config("cfg", ddl::get_buffer_config("users").repo("main"));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let row = serde_json::Value::from(resp.results["cfg"].records[0].as_value().into_owned());
    assert_eq!(row["table"], json!("users"));
    assert_eq!(row["repo"], json!("main"));
    assert!(row["config"].is_null());
}

#[tokio::test]
async fn set_then_get_buffer_config_via_ddl() {
    let shamir = setup_shamir().await;

    // Two batches -- the batch planner doesn't introduce a
    // dependency between admin ops on the same table by default,
    // so a co-batch SET + GET would race. Realistic clients
    // serialize DDL anyway.
    let mut b = Batch::new();
    b.id(1);
    b.set_buffer_config(
        "set",
        ddl::set_buffer_config("users", full_cfg()).repo("main"),
    );
    let set_resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Set echoes back the persisted config.
    let set_row =
        serde_json::Value::from(set_resp.results["set"].records[0].as_value().into_owned());
    assert_eq!(set_row["set_buffer_config"], json!("users"));
    assert_eq!(set_row["config"]["max_bytes"], json!(1_048_576));
    assert_eq!(set_row["config"]["ttl_ms"], json!(7000));

    let mut b = Batch::new();
    b.id(2);
    b.get_buffer_config("after", ddl::get_buffer_config("users").repo("main"));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let got_row = serde_json::Value::from(resp.results["after"].records[0].as_value().into_owned());
    let cfg = &got_row["config"];
    assert!(!cfg.is_null());
    assert_eq!(cfg["max_bytes"], json!(1_048_576));
    assert_eq!(cfg["max_entries"], json!(500));
    assert_eq!(cfg["ttl_ms"], json!(7000));
    assert_eq!(cfg["flush_interval_ms"], json!(333));
    assert_eq!(cfg["flush_batch_size"], json!(48));
}

#[tokio::test]
async fn alter_buffer_config_partial_update_via_ddl() {
    let shamir = setup_shamir().await;

    // Seed with a known full config first.
    let mut b = Batch::new();
    b.id(1);
    b.set_buffer_config(
        "set",
        ddl::set_buffer_config("users", full_cfg()).repo("main"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Alter ONE knob -- flush_interval_ms -- and clear ttl_ms via
    // explicit null. Other knobs must keep their seeded values.
    let mut b = Batch::new();
    b.id(2);
    b.alter_buffer_config(
        "alter",
        ddl::alter_buffer_config(
            "users",
            BufPatch {
                flush_interval_ms: Some(1000),
                ttl_ms: Some(None),
                ..Default::default()
            },
        )
        .repo("main"),
    );

    let alter_resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let alter_row = serde_json::Value::from(
        alter_resp.results["alter"].records[0]
            .as_value()
            .into_owned(),
    );
    assert_eq!(alter_row["config"]["flush_interval_ms"], json!(1000));
    assert!(alter_row["config"]["ttl_ms"].is_null());
    // Untouched knobs survived.
    assert_eq!(alter_row["config"]["max_bytes"], json!(1_048_576));
    assert_eq!(alter_row["config"]["max_entries"], json!(500));

    let mut b = Batch::new();
    b.id(3);
    b.get_buffer_config("after", ddl::get_buffer_config("users").repo("main"));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let got_row = serde_json::Value::from(resp.results["after"].records[0].as_value().into_owned());
    let got = &got_row["config"];
    assert_eq!(got["flush_interval_ms"], json!(1000));
    assert!(got["ttl_ms"].is_null());
    assert_eq!(got["max_bytes"], json!(1_048_576));
}

#[tokio::test]
async fn alter_with_omitted_ttl_keeps_existing_ttl() {
    // Confirm the three-state contract: omitting ttl_ms in the
    // patch object MUST preserve the existing TTL, not clear it.
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.set_buffer_config(
        "set",
        ddl::set_buffer_config("users", full_cfg()).repo("main"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.alter_buffer_config(
        "alter",
        ddl::alter_buffer_config(
            "users",
            BufPatch {
                max_entries: Some(9999),
                ..Default::default()
            },
        )
        .repo("main"),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let alter_row =
        serde_json::Value::from(resp.results["alter"].records[0].as_value().into_owned());
    let cfg = &alter_row["config"];
    assert_eq!(cfg["max_entries"], json!(9999));
    // ttl_ms was NOT in the patch -- must equal the seeded value.
    assert_eq!(cfg["ttl_ms"], json!(7000));
}

#[tokio::test]
async fn alter_starts_from_default_when_no_prior_config() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.alter_buffer_config(
        "alter",
        ddl::alter_buffer_config(
            "users",
            BufPatch {
                max_entries: Some(42),
                ..Default::default()
            },
        )
        .repo("main"),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let alter_row2 =
        serde_json::Value::from(resp.results["alter"].records[0].as_value().into_owned());
    let cfg = &alter_row2["config"];
    assert_eq!(cfg["max_entries"], json!(42));
    // Other fields are the engine defaults (defined in MemBufferConfig::default).
    assert_eq!(cfg["flush_interval_ms"], json!(500));
    assert_eq!(cfg["flush_batch_size"], json!(256));
}

#[tokio::test]
async fn set_buffer_config_persists_into_info_store() {
    // After SET, a fresh GET in a new batch sees the value. This
    // also verifies the executor wrote to the right info_store
    // (otherwise the second batch would not find it).
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.set_buffer_config(
        "set",
        ddl::set_buffer_config("users", full_cfg()).repo("main"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.get_buffer_config("get", ddl::get_buffer_config("users").repo("main"));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let get_row = serde_json::Value::from(resp.results["get"].records[0].as_value().into_owned());
    let cfg = &get_row["config"];
    assert_eq!(cfg["max_bytes"], json!(1_048_576));
    assert_eq!(cfg["ttl_ms"], json!(7000));
}

#[tokio::test]
async fn get_unknown_table_errors_cleanly() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.get_buffer_config("get", ddl::get_buffer_config("nonexistent").repo("main"));
    let resp = shamir.execute("testdb", &b.to_request_via_msgpack()).await;

    // Resolver/executor error surfaces -- the batch as a whole
    // fails (no `_ignore` semantics yet). Either Err(_) or an
    // empty/missing result entry both indicate the op didn't
    // silently succeed against a phantom table.
    if let Ok(r) = resp {
        assert!(
            !r.results.contains_key("get") || r.results["get"].records.is_empty(),
            "phantom table must not produce a success record"
        );
    }
}
