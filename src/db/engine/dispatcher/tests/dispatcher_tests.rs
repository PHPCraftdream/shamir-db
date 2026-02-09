use crate::db::engine::dispatcher::Dispatcher;
use crate::db::engine::repo::{RepoConfig, BoxRepo};
use crate::db::engine::table::TableConfig;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use std::sync::Arc;

#[tokio::test]
async fn test_dispatcher_new() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![
        RepoConfig::new("default", BoxRepo::InMemory(repo.clone()))
            .add_table(TableConfig::new("users"))
            .add_table(TableConfig::new("products").with_indexes()),
    ];

    let dispatcher = Dispatcher::new(configs);

    assert_eq!(dispatcher.table_count(), 2);
    assert_eq!(dispatcher.repo_count(), 1);
    assert!(dispatcher.has_table("default", "users"));
    assert!(dispatcher.has_table("default", "products"));
    assert!(!dispatcher.has_table("default", "orders"));

    let names = dispatcher.list_tables("default").unwrap();
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"products".to_string()));
}

#[tokio::test]
async fn test_get_table_lazy_loading() {
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("default", BoxRepo::InMemory(repo))
        .add_table(TableConfig::new("users"));
    let dispatcher = Dispatcher::new(vec![config]);

    let ctx1 = dispatcher.get_table("default", "users").await.unwrap();
    assert_eq!(ctx1.name(), "users");

    let ctx2 = dispatcher.get_table("default", "users").await.unwrap();
    assert_eq!(ctx2.name(), "users");

    assert_eq!(
        ctx1.name(),
        ctx2.name()
    );
}

#[tokio::test]
async fn test_get_table_not_configured() {
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("default", BoxRepo::InMemory(repo))
        .add_table(TableConfig::new("users"));
    let dispatcher = Dispatcher::new(vec![config]);

    let result = dispatcher.get_table("default", "products").await;
    assert!(result.is_err());
    match result {
        Err(crate::db::DbError::NotFound(msg)) => {
            assert!(msg.contains("products"));
            assert!(msg.contains("not configured"));
        }
        _ => panic!("Expected NotFound error"),
    }
}

#[tokio::test]
async fn test_multiple_repositories() {
    let repo1 = Arc::new(InMemoryRepo::new());
    let repo2 = Arc::new(InMemoryRepo::new());

    let configs = vec![
        RepoConfig::new("repo1", BoxRepo::InMemory(repo1))
            .add_table(TableConfig::new("users"))
            .add_table(TableConfig::new("orders")),
        RepoConfig::new("repo2", BoxRepo::InMemory(repo2))
            .add_table(TableConfig::new("products")),
    ];

    let dispatcher = Dispatcher::new(configs);

    assert_eq!(dispatcher.repo_count(), 2);

    let users = dispatcher.get_table("repo1", "users").await.unwrap();
    assert_eq!(users.name(), "users");

    let orders = dispatcher.get_table("repo1", "orders").await.unwrap();
    assert_eq!(orders.name(), "orders");

    let products = dispatcher.get_table("repo2", "products").await.unwrap();
    assert_eq!(products.name(), "products");

    assert_eq!(dispatcher.table_count(), 3);
}

#[tokio::test]
async fn test_multiple_tables() {
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("default", BoxRepo::InMemory(repo))
        .add_table(TableConfig::new("users").with_indexes())
        .add_table(TableConfig::new("products"))
        .add_table(TableConfig::new("orders"));
    let dispatcher = Dispatcher::new(vec![config]);

    let products = dispatcher.get_table("default", "products").await.unwrap();
    assert_eq!(products.name(), "products");

    let users = dispatcher.get_table("default", "users").await.unwrap();
    assert_eq!(users.name(), "users");

    let orders = dispatcher.get_table("default", "orders").await.unwrap();
    assert_eq!(orders.name(), "orders");

    assert_eq!(dispatcher.table_count(), 3);
}

#[tokio::test]
async fn test_table_context_components() {
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("default", BoxRepo::InMemory(repo))
        .add_table(TableConfig::new("users"));
    let dispatcher = Dispatcher::new(vec![config]);

    let ctx = dispatcher.get_table("default", "users").await.unwrap();

    assert_eq!(ctx.name(), "users");

    use crate::types::value::InnerValue;
    let value = InnerValue::Str("test".to_string());
    let record_id = ctx.insert(&value).await.unwrap();
    assert_eq!(ctx.count().await.unwrap(), 1);

    let retrieved = ctx.table().get(record_id).await.unwrap();
    assert_eq!(retrieved, value);
}

#[tokio::test]
async fn test_dispatcher_clone() {
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("default", BoxRepo::InMemory(repo))
        .add_table(TableConfig::new("users"));
    let dispatcher1 = Dispatcher::new(vec![config]);

    let dispatcher2 = dispatcher1.clone();

    assert_eq!(dispatcher1.repo_count(), dispatcher2.repo_count());
    assert_eq!(dispatcher1.table_count(), dispatcher2.table_count());
    assert!(dispatcher1.has_table("default", "users"));
    assert!(dispatcher2.has_table("default", "users"));

    let ctx1 = dispatcher1.get_table("default", "users").await.unwrap();
    let _ = ctx1.insert(&crate::types::value::InnerValue::Int(42)).await.unwrap();

    let ctx2 = dispatcher2.get_table("default", "users").await.unwrap();
    assert_eq!(ctx2.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_add_repo() {
    let mut dispatcher = Dispatcher::new(vec![]);

    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("new_repo", BoxRepo::InMemory(repo))
        .add_table(TableConfig::new("test_table"));

    dispatcher.add_repo(config);

    assert_eq!(dispatcher.repo_count(), 1);
    assert!(dispatcher.has_repo("new_repo"));

    let ctx = dispatcher.get_table("new_repo", "test_table").await.unwrap();
    assert_eq!(ctx.name(), "test_table");
}

#[tokio::test]
async fn test_list_repos() {
    let repo1 = Arc::new(InMemoryRepo::new());
    let repo2 = Arc::new(InMemoryRepo::new());

    let configs = vec![
        RepoConfig::new("repo1", BoxRepo::InMemory(repo1)),
        RepoConfig::new("repo2", BoxRepo::InMemory(repo2)),
    ];

    let dispatcher = Dispatcher::new(configs);

    let repos = dispatcher.list_repos();
    assert_eq!(repos.len(), 2);
    assert!(repos.contains(&"repo1".to_string()));
    assert!(repos.contains(&"repo2".to_string()));
}
