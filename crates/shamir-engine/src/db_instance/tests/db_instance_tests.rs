use crate::db_instance::db_instance::DbInstance;
use crate::repo::{BoxRepoFactory, RepoConfig};
use crate::table::TableConfig;

#[tokio::test]
async fn test_db_instance_new() {
    let configs = vec![RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("products").with_indexes())];

    let db = DbInstance::with_repos(configs).await.unwrap();

    assert_eq!(db.table_count(), 2);
    assert_eq!(db.repo_count(), 1);
    assert!(db.has_table("default", "users"));
    assert!(db.has_table("default", "products"));
    assert!(!db.has_table("default", "orders"));

    let names = db.list_tables("default").unwrap();
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"products".to_string()));
}

#[tokio::test]
async fn test_get_table_lazy_loading() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    let ctx1 = db.get_table("default", "users").await.unwrap();
    assert_eq!(ctx1.name(), "users");

    let ctx2 = db.get_table("default", "users").await.unwrap();
    assert_eq!(ctx2.name(), "users");

    assert_eq!(ctx1.name(), ctx2.name());
}

#[tokio::test]
async fn test_get_table_not_configured() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    let result = db.get_table("default", "products").await;
    assert!(result.is_err());
    match result {
        Err(shamir_storage::error::DbError::NotFound(msg)) => {
            assert!(msg.contains("products"));
            assert!(msg.contains("not configured"));
        }
        _ => panic!("Expected NotFound error"),
    }
}

#[tokio::test]
async fn test_multiple_repositories() {
    let configs = vec![
        RepoConfig::new("repo1", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users"))
            .add_table(TableConfig::new("orders")),
        RepoConfig::new("repo2", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("products")),
    ];

    let db = DbInstance::with_repos(configs).await.unwrap();

    assert_eq!(db.repo_count(), 2);

    let users = db.get_table("repo1", "users").await.unwrap();
    assert_eq!(users.name(), "users");

    let orders = db.get_table("repo1", "orders").await.unwrap();
    assert_eq!(orders.name(), "orders");

    let products = db.get_table("repo2", "products").await.unwrap();
    assert_eq!(products.name(), "products");

    assert_eq!(db.table_count(), 3);
}

#[tokio::test]
async fn test_multiple_tables() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users").with_indexes())
        .add_table(TableConfig::new("products"))
        .add_table(TableConfig::new("orders"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    let products = db.get_table("default", "products").await.unwrap();
    assert_eq!(products.name(), "products");

    let users = db.get_table("default", "users").await.unwrap();
    assert_eq!(users.name(), "users");

    let orders = db.get_table("default", "orders").await.unwrap();
    assert_eq!(orders.name(), "orders");

    assert_eq!(db.table_count(), 3);
}

#[tokio::test]
async fn test_table_context_components() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    let ctx = db.get_table("default", "users").await.unwrap();

    assert_eq!(ctx.name(), "users");

    use shamir_types::types::value::InnerValue;
    let value = InnerValue::Str("test".to_string());
    let record_id = ctx.insert(&value).await.unwrap();
    assert_eq!(ctx.count().await.unwrap(), 1);

    let retrieved = ctx.table().get(record_id).await.unwrap();
    assert_eq!(retrieved, value);
}

#[tokio::test]
async fn test_db_clone() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db1 = DbInstance::with_repos(vec![config]).await.unwrap();

    let db2 = db1.clone();

    assert_eq!(db1.repo_count(), db2.repo_count());
    assert_eq!(db1.table_count(), db2.table_count());
    assert!(db1.has_table("default", "users"));
    assert!(db2.has_table("default", "users"));

    let ctx1 = db1.get_table("default", "users").await.unwrap();
    let _ = ctx1
        .insert(&shamir_types::types::value::InnerValue::Int(42))
        .await
        .unwrap();

    let ctx2 = db2.get_table("default", "users").await.unwrap();
    assert_eq!(ctx2.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_add_repo() {
    let db = DbInstance::new();

    let config = RepoConfig::new("new_repo", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("test_table"));

    db.add_repo(config).await.unwrap();

    assert_eq!(db.repo_count(), 1);
    assert!(db.has_repo("new_repo"));

    let ctx = db.get_table("new_repo", "test_table").await.unwrap();
    assert_eq!(ctx.name(), "test_table");
}

#[tokio::test]
async fn test_list_repos() {
    let configs = vec![
        RepoConfig::new("repo1", BoxRepoFactory::in_memory()),
        RepoConfig::new("repo2", BoxRepoFactory::in_memory()),
    ];

    let db = DbInstance::with_repos(configs).await.unwrap();

    let repos = db.list_repos();
    assert_eq!(repos.len(), 2);
    assert!(repos.contains(&"repo1".to_string()));
    assert!(repos.contains(&"repo2".to_string()));
}

// ============================================================================
// Index API tests through DbInstance
// ============================================================================

#[tokio::test]
async fn test_db_create_index() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    // Create index through db
    db.create_index("default", "users", "email_idx", &["email"])
        .await
        .unwrap();

    // Check index exists
    assert!(db
        .index_exists("default", "users", "email_idx")
        .await
        .unwrap());
    assert!(!db
        .index_exists("default", "users", "nonexistent")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_db_create_composite_index() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    // Create composite index
    db.create_index("default", "users", "name_city_idx", &["name", "city"])
        .await
        .unwrap();

    assert!(db
        .index_exists("default", "users", "name_city_idx")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_db_create_unique_index() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    // Create unique index
    db.create_unique_index("default", "users", "email_unique", &["email"])
        .await
        .unwrap();

    // Check unique index exists (not regular)
    assert!(!db
        .index_exists("default", "users", "email_unique")
        .await
        .unwrap());
    assert!(db
        .unique_index_exists("default", "users", "email_unique")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_db_drop_index() {
    let config = RepoConfig::new("default", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let db = DbInstance::with_repos(vec![config]).await.unwrap();

    // Create and drop
    db.create_index("default", "users", "email_idx", &["email"])
        .await
        .unwrap();
    assert!(db
        .index_exists("default", "users", "email_idx")
        .await
        .unwrap());

    let dropped = db
        .drop_index("default", "users", "email_idx")
        .await
        .unwrap();
    assert!(dropped);
    assert!(!db
        .index_exists("default", "users", "email_idx")
        .await
        .unwrap());

    // Drop non-existent returns false
    let dropped_again = db
        .drop_index("default", "users", "email_idx")
        .await
        .unwrap();
    assert!(!dropped_again);
}

#[tokio::test]
async fn test_db_index_multiple_repos() {
    let configs = vec![
        RepoConfig::new("repo1", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users")),
        RepoConfig::new("repo2", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users")),
    ];

    let db = DbInstance::with_repos(configs).await.unwrap();

    // Create indexes in different repos
    db.create_index("repo1", "users", "email_idx", &["email"])
        .await
        .unwrap();
    db.create_index("repo2", "users", "name_idx", &["name"])
        .await
        .unwrap();

    // Check isolation
    assert!(db
        .index_exists("repo1", "users", "email_idx")
        .await
        .unwrap());
    assert!(!db.index_exists("repo1", "users", "name_idx").await.unwrap());
    assert!(!db
        .index_exists("repo2", "users", "email_idx")
        .await
        .unwrap());
    assert!(db.index_exists("repo2", "users", "name_idx").await.unwrap());
}
