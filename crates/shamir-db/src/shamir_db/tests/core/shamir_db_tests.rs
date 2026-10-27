use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

/// Deterministic proof that `db_create_lock` actually brackets
/// `handle_create_db`'s exists-check -> authorize -> create sequence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_db_lock_actually_serialises_concurrent_callers() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("router").await.expect("create_db");
    let held_guard = shamir.db_create_lock().lock().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_db("cd", ddl::create_db("lockedb"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let shamir2 = shamir.clone();
    let spawned = tokio::spawn(async move { shamir2.execute("router", &req).await });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!spawned.is_finished());
    assert!(!shamir.has_db("lockedb"));

    drop(held_guard);
    let resp = tokio::time::timeout(std::time::Duration::from_secs(5), spawned)
        .await
        .expect("spawned create_db must complete promptly after lock release")
        .expect("task must not panic")
        .expect("create_db must succeed once the lock is free");
    assert_eq!(
        resp.results["cd"].records[0].get_value_bool("created"),
        Some(true)
    );
    assert!(shamir.has_db("lockedb"));
}

/// Same deterministic proof for `repo_create_locks` (per-db keyed lock).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_repo_lock_actually_serialises_concurrent_callers() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await.expect("create_db");
    let repo_lock = shamir
        .repo_create_locks()
        .entry("testdb".to_string())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let held_guard = repo_lock.lock().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("lockedrepo").engine("in_memory"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let shamir2 = shamir.clone();
    let spawned = tokio::spawn(async move { shamir2.execute("testdb", &req).await });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!spawned.is_finished());
    let db = shamir.get_db("testdb").unwrap();
    assert!(!db.has_repo("lockedrepo"));

    drop(held_guard);
    let resp = tokio::time::timeout(std::time::Duration::from_secs(5), spawned)
        .await
        .expect("spawned create_repo must complete promptly after lock release")
        .expect("task must not panic")
        .expect("create_repo must succeed once the lock is free");
    assert_eq!(
        resp.results["cr"].records[0].get_value_bool("created"),
        Some(true)
    );
    assert!(db.has_repo("lockedrepo"));
}

#[tokio::test]
async fn test_shamir_db_creation() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    // No user databases initially
    assert_eq!(shamir.db_count(), 0);
}

#[tokio::test]
async fn test_create_db() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let _db = shamir.create_db("production").await.expect("create_db");
    assert_eq!(shamir.db_count(), 1);
    assert!(shamir.has_db("production"));

    // Creating same db again returns existing
    let _db2 = shamir.create_db("production").await.expect("create_db");
    assert_eq!(shamir.db_count(), 1);
}

#[tokio::test]
async fn test_get_db() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Get non-existent returns None
    assert!(shamir.get_db("production").is_none());

    shamir.create_db("production").await.expect("create_db");
    assert!(shamir.get_db("production").is_some());
}

#[tokio::test]
async fn test_create_db_idempotent() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.create_db("production").await.expect("create_db");
    assert_eq!(shamir.db_count(), 1);

    // Creating again doesn't duplicate
    shamir.create_db("production").await.expect("create_db");
    assert_eq!(shamir.db_count(), 1);
}

#[tokio::test]
async fn test_list_dbs() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    assert_eq!(shamir.list_dbs().len(), 0);

    shamir.create_db("production").await.expect("create_db");
    shamir.create_db("test").await.expect("create_db");
    shamir.create_db("dev").await.expect("create_db");

    let dbs = shamir.list_dbs();
    assert_eq!(dbs.len(), 3);
    assert!(dbs.contains(&"production".to_string()));
    assert!(dbs.contains(&"test".to_string()));
    assert!(dbs.contains(&"dev".to_string()));
}

#[tokio::test]
async fn test_remove_db() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.create_db("production").await.expect("create_db");
    assert_eq!(shamir.db_count(), 1);

    // Remove existing
    let removed = shamir.remove_db("production").await.unwrap();
    assert!(removed);
    assert_eq!(shamir.db_count(), 0);

    // Remove non-existent
    let removed = shamir.remove_db("production").await.unwrap();
    assert!(!removed);
}

#[tokio::test]
async fn test_remove_system_db_forbidden() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Cannot remove system DB
    let removed = shamir.remove_db("__system__").await.unwrap();
    assert!(!removed);
}

#[tokio::test]
async fn test_shamir_db_clone_shares_state() {
    let shamir1 = ShamirDb::init_memory().await.unwrap();
    shamir1.create_db("production").await.expect("create_db");

    let shamir2 = shamir1.clone();

    // Both see same state
    assert_eq!(shamir1.db_count(), shamir2.db_count());
    assert!(shamir1.has_db("production"));
    assert!(shamir2.has_db("production"));

    // Mutations are shared
    shamir2.create_db("test").await.expect("create_db");
    assert_eq!(shamir1.db_count(), 2);
    assert!(shamir1.has_db("test"));
}

// ============================================================================
// Integration tests - working with DbInstance through ShamirDb
// ============================================================================

#[tokio::test]
async fn test_db_with_repo_and_table() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("production").await.expect("create_db");

    // Configure repo with table
    let config = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));

    db.add_repo(config).await.unwrap();

    // Access table through shamir -> db -> table
    let table = db.get_table("users_db", "users").await.unwrap();
    assert_eq!(table.name(), "users");
}

#[tokio::test]
async fn test_multiple_dbs_isolation() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let db1 = shamir.create_db("production").await.expect("create_db");
    let db2 = shamir.create_db("test").await.expect("create_db");

    // Configure each db independently
    db1.add_repo(
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users")),
    )
    .await
    .unwrap();
    db2.add_repo(
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users")),
    )
    .await
    .unwrap();

    // Each db has its own table
    let table1 = db1.get_table("data", "users").await.unwrap();
    let table2 = db2.get_table("data", "users").await.unwrap();

    // Insert in db1
    use crate::types::value::InnerValue;
    table1
        .insert(&InnerValue::Str("prod_data".to_string()))
        .await
        .unwrap();

    // db2 is isolated
    assert_eq!(table2.count().await.unwrap(), 0);
    assert_eq!(table1.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_shamir_db_index_api() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("production").await.expect("create_db");

    db.add_repo(
        RepoConfig::new("users_db", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users")),
    )
    .await
    .unwrap();

    // Create index through db
    db.create_index("users_db", "users", "email_idx", &["email"])
        .await
        .unwrap();

    assert!(db
        .index_exists("users_db", "users", "email_idx")
        .await
        .unwrap());
}

// ============================================================================
// Tests for new hierarchy methods
// ============================================================================

#[tokio::test]
async fn test_remove_repo_from_shamir_db() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await.expect("create_db");

    let config = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));

    shamir.add_repo("production", config).await.unwrap();

    let db = shamir.get_db("production").unwrap();
    assert!(db.has_repo("users_db"));

    // Remove repo
    let removed = shamir.remove_repo("production", "users_db").await.unwrap();
    assert!(removed);
    assert!(!db.has_repo("users_db"));

    // Remove non-existent repo
    let removed = shamir.remove_repo("production", "users_db").await.unwrap();
    assert!(!removed);

    // Remove from non-existent db
    let removed = shamir.remove_repo("nonexistent", "users_db").await.unwrap();
    assert!(!removed);
}

#[tokio::test]
async fn test_get_table_shortcut() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await.expect("create_db");

    let config = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));

    shamir.add_repo("production", config).await.unwrap();

    // Direct table access through ShamirDb
    let table = shamir
        .get_table("production", "users_db", "users")
        .await
        .unwrap();
    assert_eq!(table.name(), "users");

    // Non-existent db
    let result = shamir.get_table("nonexistent", "users_db", "users").await;
    assert!(result.is_err());

    // Non-existent repo
    let result = shamir.get_table("production", "nonexistent", "users").await;
    assert!(result.is_err());

    // Non-existent table
    let result = shamir
        .get_table("production", "users_db", "nonexistent")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_db_instance_get_repo() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("production").await.expect("create_db");

    let config = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));

    db.add_repo(config).await.unwrap();

    // Get repo
    let repo_instance = db.get_repo("users_db");
    assert!(repo_instance.is_some());

    // Non-existent repo
    let repo_instance = db.get_repo("nonexistent");
    assert!(repo_instance.is_none());
}

#[tokio::test]
async fn test_db_instance_remove_repo() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("production").await.expect("create_db");

    let config = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));

    db.add_repo(config).await.unwrap();
    assert!(db.has_repo("users_db"));

    // Remove repo
    let removed = db.remove_repo("users_db").await;
    assert!(removed);
    assert!(!db.has_repo("users_db"));

    // Remove non-existent repo
    let removed = db.remove_repo("users_db").await;
    assert!(!removed);
}
