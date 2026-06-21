use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write;
use shamir_query_builder::Query;

use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::ShamirDb;
use crate::shamir_db::SystemStoreConfig;
use shamir_types::types::value::QueryValue;

// ============================================================================
// System store persistence tests
// ============================================================================

#[tokio::test]
async fn test_create_db_persists() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.create_db("production").await;
    assert!(shamir.has_db("production"));

    // Verify persisted in system store
    let dbs = shamir.system_store().load_databases().await.unwrap();
    assert!(dbs.iter().any(|d| d["name"] == "production"));
}

#[tokio::test]
async fn test_remove_db_removes_from_system_store() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.create_db("temp").await;
    shamir.remove_db("temp").await;

    let dbs = shamir.system_store().load_databases().await.unwrap();
    assert!(!dbs.iter().any(|d| d["name"] == "temp"));
}

#[tokio::test]
async fn test_add_repo_persists() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await;

    let config = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    shamir.add_repo("production", config).await.unwrap();

    let repos = shamir.system_store().load_repositories().await.unwrap();
    assert!(repos
        .iter()
        .any(|r| r["repo_name"] == "users_db" && r["db_name"] == "production"));
}

#[tokio::test]
async fn test_remove_repo_removes_from_system_store() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await;

    let config = RepoConfig::new("temp_repo", BoxRepoFactory::in_memory());
    shamir.add_repo("production", config).await.unwrap();
    shamir.remove_repo("production", "temp_repo").await;

    let repos = shamir.system_store().load_repositories().await.unwrap();
    assert!(!repos.iter().any(|r| r["repo_name"] == "temp_repo"));
}

#[tokio::test]
async fn test_system_store_has_tables() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // System store should have settings, users, roles tables accessible
    let settings = shamir
        .system_store()
        .load_setting("nonexistent")
        .await
        .unwrap();
    assert!(settings.is_none());
}

#[tokio::test]
async fn test_settings_persistence() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir
        .system_store()
        .save_setting("max_connections", &QueryValue::Int(100))
        .await
        .unwrap();

    let val = shamir
        .system_store()
        .load_setting("max_connections")
        .await
        .unwrap();
    assert_eq!(val, Some(QueryValue::Int(100)));
}

#[tokio::test]
async fn test_multiple_repos_persist() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await;

    let config1 = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let config2 = RepoConfig::new("products_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("products"));

    shamir.add_repo("production", config1).await.unwrap();
    shamir.add_repo("production", config2).await.unwrap();

    let repos = shamir.system_store().load_repositories().await.unwrap();
    let prod_repos: Vec<_> = repos
        .iter()
        .filter(|r| r["db_name"] == "production")
        .collect();
    assert_eq!(prod_repos.len(), 2);
}

// ============================================================================
// I.2 — table catalogue persistence (recovery data-replay enablement)
// ============================================================================

/// Count records returned by a `{"from": <table>}` read through the
/// high-level execute pipeline.
async fn read_count(shamir: &ShamirDb, db: &str, repo: &str, table: &str) -> usize {
    let mut b = Batch::new();
    b.id(1);
    b.query("q", Query::with_repo(repo, table));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute(db, &req).await.unwrap();
    resp.results["q"].records.len()
}

/// Re-open the system store, retrying briefly while the previous session's
/// store still holds the redb file lock.
///
/// The system store is MemBuffer-wrapped, whose background flusher is woken
/// immediately on drop (`Notify::notify_one`) but releases the underlying
/// redb lock only once it has drained and exited — a few milliseconds
/// *after* the owning `ShamirDb` is dropped. Polling here is deterministic:
/// it returns as soon as the lock is free, and the bound only guards
/// against a genuinely stuck open. This is a test artifact of dropping and
/// re-opening the same on-disk store inside one process; production opens
/// the store exactly once per run.
async fn reinit_with_retry(sys_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .expect("system store still locked after retries")
}

/// I.2 core proof: a repo + table created over a disk-backed (redb) store
/// must still exist after the ShamirDb is dropped and re-initialised over
/// the SAME underlying store — WITHOUT re-creating the table — and its
/// data must be readable. Before this fix `init` re-attached repos with an
/// empty table list, so the table didn't exist on restart.
#[tokio::test]
async fn table_catalogue_survives_restart() {
    let sys_dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");
    let repo_path = repo_dir.path().join("data.fjall");

    // === Session 1: create repo + table, write a record ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("production").await;

        // `fjall_raw` (unbuffered): writes are durable on return and the
        // store releases synchronously on drop, so the repo store is
        // immediately re-openable by session 2.
        let config = RepoConfig::new("data", BoxRepoFactory::fjall_raw(repo_path.clone()))
            .add_table(TableConfig::new("users"));
        shamir.add_repo("production", config).await.unwrap();

        // Durable write through the high-level execute pipeline (execute_set
        // persists interner + counter).
        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins",
            write::Insert::with_repo("data", "users").row(doc! {
                "name" => "Alice",
                "email" => "alice@example.com",
            }),
        );
        let ins = b.to_request_via_msgpack();
        let resp = shamir.execute("production", &ins).await.unwrap();
        assert_eq!(resp.results["ins"].records.len(), 1);

        assert_eq!(read_count(&shamir, "production", "data", "users").await, 1);
    }

    // === Session 2: re-init over the SAME store ===
    let shamir = reinit_with_retry(sys_path).await;

    // The table must EXIST after init, without anyone re-creating it.
    let db = shamir.get_db("production").expect("db restored");
    assert!(
        db.has_repo("data"),
        "repo should be re-attached from system store"
    );
    assert!(
        db.has_table("data", "users"),
        "table catalogue must be restored on init (I.2)"
    );

    // And its data must be readable through the restored table.
    assert_eq!(
        read_count(&shamir, "production", "data", "users").await,
        1,
        "row written before restart must be readable after restart"
    );
}

/// A table created via `ShamirDb::add_table` (the path the executor's
/// `CreateTable` now routes through) must survive a restart.
#[tokio::test]
async fn table_added_after_repo_survives_restart() {
    let sys_dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");
    let repo_path = repo_dir.path().join("data.fjall");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("production").await;

        // Repo created with NO inline tables (fjall_raw for synchronous
        // release on drop).
        let config = RepoConfig::new("data", BoxRepoFactory::fjall_raw(repo_path.clone()));
        shamir.add_repo("production", config).await.unwrap();

        // Table added afterwards — persisted via add_table.
        shamir
            .add_table("production", "data", "events", false)
            .await
            .unwrap();

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins",
            write::Insert::with_repo("data", "events").row(doc! {
                "kind" => "click",
            }),
        );
        let ins = b.to_request_via_msgpack();
        shamir.execute("production", &ins).await.unwrap();
    }

    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("production").expect("db restored");
    assert!(
        db.has_table("data", "events"),
        "table added via add_table must be restored after restart"
    );
    assert_eq!(read_count(&shamir, "production", "data", "events").await, 1);
}

/// A table dropped via `ShamirDb::drop_table` must NOT reappear after a
/// restart — its catalogue entry must be removed.
#[tokio::test]
async fn dropped_table_does_not_resurrect_after_restart() {
    let sys_dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");
    let repo_path = repo_dir.path().join("data.fjall");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("production").await;

        let config = RepoConfig::new("data", BoxRepoFactory::fjall_raw(repo_path.clone()))
            .add_table(TableConfig::new("keep"))
            .add_table(TableConfig::new("scratch"));
        shamir.add_repo("production", config).await.unwrap();

        // Drop one of the two tables.
        let removed = shamir
            .drop_table("production", "data", "scratch")
            .await
            .unwrap();
        assert!(removed);
    }

    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("production").expect("db restored");
    assert!(
        db.has_table("data", "keep"),
        "surviving table must still be present"
    );
    assert!(
        !db.has_table("data", "scratch"),
        "dropped table must not resurrect after restart"
    );
}

/// End-to-end I.2 catalogue persistence check via the system_store API:
/// the per-table records are actually written and reloaded.
#[tokio::test]
async fn table_catalogue_records_are_persisted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await;

    let config = RepoConfig::new("data", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("orders"));
    shamir.add_repo("production", config).await.unwrap();

    let tables = shamir.system_store().load_tables().await.unwrap();
    let names: Vec<&str> = tables
        .iter()
        .filter(|t| t["db_name"] == "production" && t["repo_name"] == "data")
        .filter_map(|t| t["table_name"].as_str())
        .collect();
    assert!(names.contains(&"users"));
    assert!(names.contains(&"orders"));
}
