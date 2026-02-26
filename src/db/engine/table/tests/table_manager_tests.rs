use crate::db::engine::dispatcher::dispatcher::Dispatcher;
use crate::db::engine::repo::repo_types::BoxRepo;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::TableConfig;
use crate::db::engine::table::TableManager;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use crate::db::storage::types::Repo;
use crate::types::value::InnerValue;
use std::sync::Arc;

#[tokio::test]
async fn test_table_manager_creation() {
    let repo = Arc::new(InMemoryRepo::new());
    let data_store = repo.store_get("__data__test".to_string()).await.unwrap();
    let info_store = repo.store_get("__info__test".to_string()).await.unwrap();

    let data_store: Arc<dyn crate::db::storage::types::Store> = data_store;
    let info_store: Arc<dyn crate::db::storage::types::Store> = info_store;

    use crate::db::engine::index::index_manager::IndexManager;
    use crate::db::engine::table::interner_manager::InternerManager;
    use crate::db::engine::table::record_counter::RecordCounter;

    let interner = InternerManager::new(Arc::clone(&info_store));
    let counter = Arc::new(RecordCounter::new(Arc::clone(&info_store)));
    let index_manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    use crate::db::engine::table::Table;
    let table = Table::new(Arc::clone(&data_store));

    let ctx = TableManager::new("test".to_string(), table, interner, counter, index_manager);
    assert_eq!(ctx.name(), "test");
}

#[tokio::test]
async fn test_table_manager_clone() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let ctx1 = dispatcher.get_table("default", "users").await.unwrap();
    let ctx2 = ctx1.clone();

    assert_eq!(ctx1.name(), ctx2.name());
    assert!(std::ptr::eq(ctx1.table(), ctx2.table()));
}

#[tokio::test]
async fn test_table_manager_components() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let ctx = dispatcher.get_table("default", "users").await.unwrap();

    assert_eq!(ctx.name(), "users");

    let value = InnerValue::Str("test".to_string());
    let record_id = ctx.insert(&value).await.unwrap();
    assert_eq!(ctx.count().await.unwrap(), 1);

    let retrieved = ctx.table().get(record_id).await.unwrap();
    assert_eq!(retrieved, value);
}

// ============================================================================
// Index API tests (string paths)
// ============================================================================

#[tokio::test]
async fn test_create_index_simple() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let table = dispatcher.get_table("default", "users").await.unwrap();

    // Create index with string path
    table.create_index("email_idx", &["email"]).await.unwrap();

    // Check index exists
    assert!(table.index_exists("email_idx").await);
    assert!(!table.index_exists("nonexistent").await);
}

#[tokio::test]
async fn test_create_index_composite() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let table = dispatcher.get_table("default", "users").await.unwrap();

    // Create composite index
    table
        .create_index("name_city_idx", &["name", "city"])
        .await
        .unwrap();

    assert!(table.index_exists("name_city_idx").await);
}

#[tokio::test]
async fn test_create_index_nested_path() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let table = dispatcher.get_table("default", "users").await.unwrap();

    // Create index with nested path
    table
        .create_index("city_idx", &["address.city"])
        .await
        .unwrap();

    assert!(table.index_exists("city_idx").await);
}

#[tokio::test]
async fn test_drop_index() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let table = dispatcher.get_table("default", "users").await.unwrap();

    // Create and drop
    table.create_index("email_idx", &["email"]).await.unwrap();
    assert!(table.index_exists("email_idx").await);

    let dropped = table.drop_index("email_idx").await.unwrap();
    assert!(dropped);
    assert!(!table.index_exists("email_idx").await);

    // Drop non-existent returns false
    let dropped_again = table.drop_index("email_idx").await.unwrap();
    assert!(!dropped_again);
}

#[tokio::test]
async fn test_unique_index() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let table = dispatcher.get_table("default", "users").await.unwrap();

    // Create unique index
    table
        .create_unique_index("email_unique", &["email"])
        .await
        .unwrap();

    // Check unique index exists (not regular index)
    assert!(!table.index_exists("email_unique").await);
    assert!(table.unique_index_exists("email_unique").await);
}

#[tokio::test]
async fn test_lookup_by_index() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let table = dispatcher.get_table("default", "users").await.unwrap();

    // Create index
    table.create_index("status_idx", &["status"]).await.unwrap();

    // Lookup with no data returns empty
    let results = table
        .lookup_by_index("status_idx", &[InnerValue::Str("active".to_string())])
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_get_record() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        repo: BoxRepo::InMemory(repo),
        tables: configs,
    };
    let dispatcher = Dispatcher::new(vec![repo_config]);

    let table = dispatcher.get_table("default", "users").await.unwrap();

    // Insert and retrieve
    let value = InnerValue::Str("hello".to_string());
    let id = table.insert(&value).await.unwrap();

    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, value);
}
