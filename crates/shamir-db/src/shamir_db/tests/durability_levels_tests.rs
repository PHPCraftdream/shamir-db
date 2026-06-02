//! Tests for per-request durability levels (v1: `buffered` vs `synced`).

use serde_json::json;

use crate::engine::query::batch::BatchRequest;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::SystemStoreConfig;
use crate::ShamirDb;

async fn reinit_with_retry(sys_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Redb(sys_path))
        .await
        .expect("system store still locked after retries")
}

/// A non-transactional insert batch with `durability: "synced"` must
/// survive an *immediate* drop (no `flush_all`, no 500 ms tick). The
/// synced path flushes every repo's buffers before building the
/// BatchResponse, so the data is on disk before the caller receives
/// the ack.
#[tokio::test]
async fn synced_batch_survives_immediate_drop() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let repo_path = dir.path().join("data.redb");

    // === Session 1: insert with durability=synced, then DROP immediately ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let config = RepoConfig::new("data", BoxRepoFactory::redb(&repo_path))
            .add_table(TableConfig::new("items"));
        shamir.add_repo("appdb", config).await.unwrap();

        let insert: BatchRequest = serde_json::from_value(json!({
            "id": 1,
            "durability": "synced",
            "queries": {
                "ins": {
                    "insert_into": ["data", "items"],
                    "values": [
                        {
                            "name": "widget",
                            "qty": 42
                        }
                    ]
                }
            }
        }))
        .unwrap();
        let resp = shamir.execute("appdb", &insert).await.unwrap();
        assert_eq!(resp.results["ins"].records.len(), 1);

        // DROP immediately — no flush_all, no tick. synced already flushed.
    }

    // === Session 2: reopen, read back → record PRESENT ===
    let shamir = reinit_with_retry(sys_path).await;

    let read: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "r": {
                "from": ["data", "items"]
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("appdb", &read).await.unwrap();
    let records = &resp.results["r"].records;
    assert_eq!(records.len(), 1, "synced batch must survive immediate drop");
    assert_eq!(records[0]["name"], "widget");
    assert_eq!(records[0]["qty"], 42);
}

/// A non-transactional insert batch with `durability` absent (i.e.
/// `"buffered"`, the default) executes successfully and returns the
/// inserted records. Whether the data survives an immediate drop
/// depends on the MemBuffer's background flusher, which is notified
/// on every write and may drain before the owning `ShamirDb` is
/// dropped — making a loss-on-drop assertion non-deterministic.
/// The synced test above is the deterministic guarantee; this test
/// verifies the default path is accepted and produces correct results.
#[tokio::test]
async fn buffered_batch_executes_successfully() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let repo_path = dir.path().join("data.redb");

    let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path))
        .await
        .unwrap();
    shamir.create_db("appdb").await;

    let config = RepoConfig::new("data", BoxRepoFactory::redb(&repo_path))
        .add_table(TableConfig::new("items"));
    shamir.add_repo("appdb", config).await.unwrap();

    // durability absent → buffered (default).
    let insert: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": ["data", "items"],
                "values": [
                    {
                        "name": "widget",
                        "qty": 42
                    }
                ]
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("appdb", &insert).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 1);
    assert_eq!(resp.results["ins"].records[0]["name"], "widget");
    assert_eq!(resp.results["ins"].records[0]["qty"], 42);

    shamir.flush_all().await.unwrap();
}
