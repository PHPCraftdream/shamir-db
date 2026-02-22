use crate::db::engine::dispatcher::dispatcher::Dispatcher;
use crate::db::engine::repo::repo_types::BoxRepo;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::TableConfig;
use crate::db::engine::table::TableContext;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use crate::db::storage::types::Repo;
use crate::types::value::InnerValue;
use std::sync::Arc;

#[tokio::test]
async fn test_table_context_creation() {
    let repo = Arc::new(InMemoryRepo::new());
    let data_store = repo.store_get("__data__test".to_string()).await.unwrap();
    let info_store = repo.store_get("__info__test".to_string()).await.unwrap();

    let data_store: Arc<dyn crate::db::storage::types::Store> = Arc::from(data_store);
    let info_store: Arc<dyn crate::db::storage::types::Store> = Arc::from(info_store);

    use crate::db::engine::index::table_index_manager::TableIndexManager;
    use crate::db::engine::table::interner_manager::InternerManager;
    use crate::db::engine::table::record_counter::RecordCounter;

    let interner = InternerManager::new(Arc::clone(&info_store));
    let counter = Arc::new(RecordCounter::new(Arc::clone(&info_store)));
    let index_manager = TableIndexManager::new(
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    use crate::db::engine::table::Table;
    let table = Table::new(Arc::clone(&data_store));

    let ctx = TableContext::new("test".to_string(), table, interner, counter, index_manager);
    assert_eq!(ctx.name(), "test");
}

#[tokio::test]
async fn test_table_context_clone() {
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
async fn test_table_context_components() {
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
