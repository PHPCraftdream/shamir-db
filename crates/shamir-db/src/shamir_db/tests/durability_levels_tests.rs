//! Tests for per-request durability levels (v1: `buffered` vs `synced`).

use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::doc;
use shamir_query_builder::write;
use shamir_query_builder::Query;

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

        let mut b = Batch::new();
        b.id(1);
        b.durability(Durability::Synced);
        b.insert(
            "ins",
            write::Insert::with_repo("data", "items").row(doc! {
                "name" => "widget",
                "qty" => 42,
            }),
        );
        let insert = b.to_request_via_msgpack();
        let resp = shamir.execute("appdb", &insert).await.unwrap();
        assert_eq!(resp.results["ins"].records.len(), 1);

        // DROP immediately — no flush_all, no tick. synced already flushed.
    }

    // === Session 2: reopen, read back → record PRESENT ===
    let shamir = reinit_with_retry(sys_path).await;

    let mut b = Batch::new();
    b.id(2);
    b.query("r", Query::with_repo("data", "items"));
    let read = b.to_request_via_msgpack();
    let resp = shamir.execute("appdb", &read).await.unwrap();
    let records = &resp.results["r"].records;
    assert_eq!(records.len(), 1, "synced batch must survive immediate drop");
    let r0 = records[0].as_json();
    assert_eq!(r0.get("name").and_then(|v| v.as_str()), Some("widget"));
    assert_eq!(r0.get("qty").and_then(|v| v.as_i64()), Some(42));
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
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::Insert::with_repo("data", "items").row(doc! {
            "name" => "widget",
            "qty" => 42,
        }),
    );
    let insert = b.to_request_via_msgpack();
    let resp = shamir.execute("appdb", &insert).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 1);
    assert_eq!(resp.results["ins"].records[0]["name"], "widget");
    assert_eq!(resp.results["ins"].records[0]["qty"], 42);

    shamir.flush_all().await.unwrap();
}
