//! Durable-by-default tests for wire-created repos.
//!
//! Verifies that repositories created over the wire (via `CreateRepo` DDL)
//! survive a `ShamirDb` restart when the system store is durable, and that
//! explicit `engine: "in_memory"` remains ephemeral even in a durable home.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write;
use shamir_query_builder::Query;

use crate::shamir_db::SystemStoreConfig;
use crate::ShamirDb;

/// Re-open the system store, retrying briefly while the previous session's
/// store still holds the redb file lock (the MemBuffer-wrapped store releases
/// the lock a few ms after the owning `ShamirDb` is dropped). Mirrors the
/// helper in `execute_tests`.
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

/// A repo created via the wire `CreateRepo` op (engine NOT specified) with a
/// durable home must survive a full drop-and-reopen cycle. Data inserted in
/// session 1 must be readable in session 2 — proving both that the redb file
/// was created on disk *and* the catalogue record was persisted correctly.
#[tokio::test]
async fn wire_created_repo_is_durable_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: create db + repo (no engine specified) + insert ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo("cr", ddl::create_repo("data").tables(["items"]));
        let create = b.to_request_via_msgpack();
        let resp = shamir.execute("appdb", &create).await.unwrap();
        assert_eq!(resp.results["cr"].records[0]["created_repo"], "data");

        let mut b = Batch::new();
        b.id(2);
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
    }

    // === Session 2: reopen on the SAME meta path, read back ===
    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("appdb").expect("db must survive restart");
    assert!(
        db.has_repo("data"),
        "durable repo must be re-attached after restart"
    );

    let mut b = Batch::new();
    b.id(3);
    b.query("r", Query::with_repo("data", "items"));
    let read = b.to_request_via_msgpack();
    let resp = shamir.execute("appdb", &read).await.unwrap();
    let records = &resp.results["r"].records;
    assert_eq!(
        records.len(),
        1,
        "data must survive restart in a durable repo"
    );
    let r0 = records[0].as_json();
    assert_eq!(r0.get("name").and_then(|v| v.as_str()), Some("widget"));
    assert_eq!(r0.get("qty").and_then(|v| v.as_i64()), Some(42));
}

/// When the caller explicitly requests `engine: "in_memory"` inside a durable
/// home, the repo is ephemeral: data does NOT survive a restart. This test
/// verifies the opt-in ephemeral escape hatch works correctly.
#[tokio::test]
async fn explicit_in_memory_repo_is_ephemeral() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: create in_memory repo explicitly, insert ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo(
            "cr",
            ddl::create_repo("scratch")
                .engine("in_memory")
                .tables(["tmp"]),
        );
        let create = b.to_request_via_msgpack();
        shamir.execute("appdb", &create).await.unwrap();

        let mut b = Batch::new();
        b.id(2);
        b.insert(
            "ins",
            write::Insert::with_repo("scratch", "tmp").row(doc! {
                "val" => "gone",
            }),
        );
        let insert = b.to_request_via_msgpack();
        let resp = shamir.execute("appdb", &insert).await.unwrap();
        assert_eq!(resp.results["ins"].records.len(), 1);
    }

    // === Session 2: repo is re-attached (catalogue persisted), but data is gone ===
    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("appdb").expect("db must survive restart");
    assert!(
        db.has_repo("scratch"),
        "ephemeral repo catalogue must survive restart"
    );

    let mut b = Batch::new();
    b.id(3);
    b.query("r", Query::with_repo("scratch", "tmp"));
    let read = b.to_request_via_msgpack();
    let resp = shamir.execute("appdb", &read).await.unwrap();
    assert_eq!(
        resp.results["r"].records.len(),
        0,
        "in_memory repo data must NOT survive restart"
    );
}

/// A durable wire-created repo "data" in db "appdb" must produce a file at
/// `<data_root>/appdb/data.redb` on disk, mirroring the db→repo tree.
#[tokio::test]
async fn durable_repo_file_mirrors_db_repo_tree() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
        .await
        .unwrap();
    shamir.create_db("appdb").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("data").tables(["items"]));
    let create = b.to_request_via_msgpack();
    shamir.execute("appdb", &create).await.unwrap();

    let expected = dir.path().join("appdb").join("data.redb");
    assert!(
        expected.exists(),
        "durable repo file must exist at {}",
        expected.display()
    );
}
