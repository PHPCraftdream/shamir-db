//! Integration tests for per-table buffer-config DDL via
//! `ShamirDb::execute`. Exercises the JSON wire format that the
//! TCP/WS client (Node, etc.) actually sends.

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

fn full_cfg() -> serde_json::Value {
    json!({
        "max_bytes": 1_048_576usize,
        "max_entries": 500usize,
        "ttl_ms": 7000u64,
        "flush_interval_ms": 333u64,
        "flush_batch_size": 48usize,
    })
}

#[tokio::test]
async fn get_buffer_config_returns_null_when_unset() {
    let shamir = setup_shamir().await;
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cfg": { "get_buffer_config": "users", "repo": "main" }
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let row = &resp.results["cfg"].records[0];
    assert_eq!(row["table"], json!("users"));
    assert_eq!(row["repo"], json!("main"));
    assert!(row["config"].is_null());
}

#[tokio::test]
async fn set_then_get_buffer_config_via_ddl() {
    let shamir = setup_shamir().await;

    // Two batches — the batch planner doesn't introduce a
    // dependency between admin ops on the same table by default,
    // so a co-batch SET + GET would race. Realistic clients
    // serialize DDL anyway.
    let set_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "set": { "set_buffer_config": "users", "repo": "main", "config": full_cfg() }
        }
    }))
    .unwrap();
    let set_resp = shamir.execute("testdb", &set_req).await.unwrap();

    // Set echoes back the persisted config.
    let set_row = &set_resp.results["set"].records[0];
    assert_eq!(set_row["set_buffer_config"], json!("users"));
    assert_eq!(set_row["config"]["max_bytes"], json!(1_048_576));
    assert_eq!(set_row["config"]["ttl_ms"], json!(7000));

    let get_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "after": { "get_buffer_config": "users", "repo": "main" }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &get_req).await.unwrap();

    let got_row = &resp.results["after"].records[0];
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
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "set": { "set_buffer_config": "users", "repo": "main", "config": full_cfg() }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Alter ONE knob — flush_interval_ms — and clear ttl_ms via
    // explicit null. Other knobs must keep their seeded values.
    let alter: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "alter": {
                "alter_buffer_config": "users",
                "repo": "main",
                "patch": {
                    "flush_interval_ms": 1000,
                    "ttl_ms": null
                }
            }
        }
    }))
    .unwrap();

    let alter_resp = shamir.execute("testdb", &alter).await.unwrap();
    let alter_row = &alter_resp.results["alter"].records[0];
    assert_eq!(alter_row["config"]["flush_interval_ms"], json!(1000));
    assert!(alter_row["config"]["ttl_ms"].is_null());
    // Untouched knobs survived.
    assert_eq!(alter_row["config"]["max_bytes"], json!(1_048_576));
    assert_eq!(alter_row["config"]["max_entries"], json!(500));

    let get: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "after": { "get_buffer_config": "users", "repo": "main" }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &get).await.unwrap();
    let got = &resp.results["after"].records[0]["config"];
    assert_eq!(got["flush_interval_ms"], json!(1000));
    assert!(got["ttl_ms"].is_null());
    assert_eq!(got["max_bytes"], json!(1_048_576));
}

#[tokio::test]
async fn alter_with_omitted_ttl_keeps_existing_ttl() {
    // Confirm the three-state contract: omitting ttl_ms in the
    // patch object MUST preserve the existing TTL, not clear it.
    let shamir = setup_shamir().await;

    shamir
        .execute(
            "testdb",
            &serde_json::from_value::<BatchRequest>(json!({
                "id": 1,
                "queries": {
                    "set": { "set_buffer_config": "users", "repo": "main", "config": full_cfg() }
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let resp = shamir
        .execute(
            "testdb",
            &serde_json::from_value::<BatchRequest>(json!({
                "id": 2,
                "queries": {
                    "alter": {
                        "alter_buffer_config": "users",
                        "repo": "main",
                        "patch": { "max_entries": 9999 }
                    }
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let cfg = &resp.results["alter"].records[0]["config"];
    assert_eq!(cfg["max_entries"], json!(9999));
    // ttl_ms was NOT in the patch — must equal the seeded value.
    assert_eq!(cfg["ttl_ms"], json!(7000));
}

#[tokio::test]
async fn alter_starts_from_default_when_no_prior_config() {
    let shamir = setup_shamir().await;

    let resp = shamir
        .execute(
            "testdb",
            &serde_json::from_value::<BatchRequest>(json!({
                "id": 1,
                "queries": {
                    "alter": {
                        "alter_buffer_config": "users",
                        "repo": "main",
                        "patch": { "max_entries": 42 }
                    }
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let cfg = &resp.results["alter"].records[0]["config"];
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

    shamir
        .execute(
            "testdb",
            &serde_json::from_value::<BatchRequest>(json!({
                "id": 1,
                "queries": {
                    "set": { "set_buffer_config": "users", "repo": "main", "config": full_cfg() }
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let resp = shamir
        .execute(
            "testdb",
            &serde_json::from_value::<BatchRequest>(json!({
                "id": 2,
                "queries": {
                    "get": { "get_buffer_config": "users", "repo": "main" }
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let cfg = &resp.results["get"].records[0]["config"];
    assert_eq!(cfg["max_bytes"], json!(1_048_576));
    assert_eq!(cfg["ttl_ms"], json!(7000));
}

#[tokio::test]
async fn get_unknown_table_errors_cleanly() {
    let shamir = setup_shamir().await;

    let resp = shamir
        .execute(
            "testdb",
            &serde_json::from_value::<BatchRequest>(json!({
                "id": 1,
                "queries": {
                    "get": { "get_buffer_config": "nonexistent", "repo": "main" }
                }
            }))
            .unwrap(),
        )
        .await;

    // Resolver/executor error surfaces — the batch as a whole
    // fails (no `_ignore` semantics yet). Either Err(_) or an
    // empty/missing result entry both indicate the op didn't
    // silently succeed against a phantom table.
    match resp {
        Ok(r) => {
            assert!(
                !r.results.contains_key("get") || r.results["get"].records.is_empty(),
                "phantom table must not produce a success record"
            );
        }
        Err(_) => {}
    }
}
