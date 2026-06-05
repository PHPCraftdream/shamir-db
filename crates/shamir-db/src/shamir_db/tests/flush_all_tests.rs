//! Tests for ShamirDb::flush_all — draining repo MemBuffers on graceful
//! shutdown so buffered commits survive a drop-and-reopen cycle.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write;
use shamir_query_builder::Query;

use crate::engine::query::batch::BatchRequest;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::SystemStoreConfig;
use crate::ShamirDb;

fn to_req(b: &Batch) -> BatchRequest {
    let bytes = b.to_msgpack().expect("msgpack encode");
    rmp_serde::from_slice(&bytes).expect("msgpack decode")
}

/// Re-open the system store, retrying briefly while the previous session's
/// store still holds the redb file lock (the MemBuffer-wrapped store releases
/// the lock a few ms after the owning `ShamirDb` is dropped).
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

/// A buffered commit (autocommit / execute) in a redb+MemBuffer repo is NOT
/// flushed to disk by default — only the ~500 ms background tick or an
/// explicit `flush()` drains the buffer. This test proves that `flush_all`
/// drains every repo's buffers so the data survives a full drop-and-reopen
/// cycle, closing the buffered-commit loss window on graceful shutdown.
#[tokio::test]
async fn buffered_commit_survives_graceful_flush_all() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let repo_path = dir.path().join("data.redb");

    // === Session 1: create durable repo + table, insert, flush_all, drop ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let config = RepoConfig::new("data", BoxRepoFactory::redb(&repo_path))
            .add_table(TableConfig::new("items"));
        shamir.add_repo("appdb", config).await.unwrap();

        // Insert via execute (autocommit — buffered, NOT flushed by default).
        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins",
            write::Insert::with_repo("data", "items").row(doc! {
                "name" => "widget",
                "qty" => 42,
            }),
        );
        let insert = to_req(&b);
        let resp = shamir.execute("appdb", &insert).await.unwrap();
        assert_eq!(resp.results["ins"].records.len(), 1);

        // Flush all buffers to durable backing — the shutdown drain path.
        shamir.flush_all().await.unwrap();
    }

    // === Session 2: reopen on the SAME meta path, read back ===
    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("appdb").expect("db must survive restart");
    assert!(
        db.has_repo("data"),
        "durable repo must be re-attached after restart"
    );

    let mut b = Batch::new();
    b.id(2);
    b.query("r", Query::with_repo("data", "items"));
    let read = to_req(&b);
    let resp = shamir.execute("appdb", &read).await.unwrap();
    let records = &resp.results["r"].records;
    assert_eq!(
        records.len(),
        1,
        "data must survive restart after flush_all"
    );
    assert_eq!(records[0]["name"], "widget");
    assert_eq!(records[0]["qty"], 42);
}

/// `flush_all` on an in-memory home must return Ok without panicking —
/// flush on a non-durable store is a harmless no-op.
#[tokio::test]
async fn flush_all_is_safe_on_in_memory_home() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let config =
        RepoConfig::new("scratch", BoxRepoFactory::in_memory()).add_table(TableConfig::new("tmp"));
    shamir.add_repo("testdb", config).await.unwrap();

    // Insert a record so the table's stores are materialised.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::Insert::with_repo("scratch", "tmp").row(doc! {
            "val" => "hello",
        }),
    );
    let insert = to_req(&b);
    let resp = shamir.execute("testdb", &insert).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 1);

    // flush_all must succeed — no panic, no error.
    shamir.flush_all().await.unwrap();
}
