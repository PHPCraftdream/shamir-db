use crate::db::engine::dispatcher::Dispatcher;
use crate::db::engine::table::TableConfig;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use std::sync::Arc;

#[tokio::test]
async fn test_dispatcher_new() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![
        TableConfig::new("users"),
        TableConfig::new("products").with_indexes(),
    ];

    let dispatcher = Dispatcher::new(repo, configs);

    assert_eq!(dispatcher.table_count(), 2);
    assert!(dispatcher.has_table("users"));
    assert!(dispatcher.has_table("products"));
    assert!(!dispatcher.has_table("orders"));

    let names = dispatcher.list_table_names();
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"products".to_string()));
}

#[tokio::test]
async fn test_get_table_lazy_loading() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let dispatcher = Dispatcher::new(repo, configs);

    let ctx1 = dispatcher.get_table("users").await.unwrap();
    assert_eq!(ctx1.name(), "users");

    let ctx2 = dispatcher.get_table("users").await.unwrap();
    assert_eq!(ctx2.name(), "users");

    assert_eq!(
        ctx1.name(),
        ctx2.name()
    );
}

#[tokio::test]
async fn test_get_table_not_configured() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let dispatcher = Dispatcher::new(repo, configs);

    let result = dispatcher.get_table("products").await;
    assert!(result.is_err());
    match result {
        Err(crate::db::error::DbError::NotFound(msg)) => {
            assert!(msg.contains("products"));
            assert!(msg.contains("not configured"));
        }
        _ => panic!("Expected NotFound error"),
    }
}

#[tokio::test]
async fn test_multiple_tables() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![
        TableConfig::new("users").with_indexes(),
        TableConfig::new("products"),
        TableConfig::new("orders"),
    ];
    let dispatcher = Dispatcher::new(repo, configs);

    let products = dispatcher.get_table("products").await.unwrap();
    assert_eq!(products.name(), "products");

    let users = dispatcher.get_table("users").await.unwrap();
    assert_eq!(users.name(), "users");

    let orders = dispatcher.get_table("orders").await.unwrap();
    assert_eq!(orders.name(), "orders");

    assert_eq!(dispatcher.table_count(), 3);
}

#[tokio::test]
async fn test_table_context_components() {
    let repo = Arc::new(InMemoryRepo::new());
    let configs = vec![TableConfig::new("users")];
    let dispatcher = Dispatcher::new(repo, configs);

    let ctx = dispatcher.get_table("users").await.unwrap();

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
    let configs = vec![TableConfig::new("users")];
    let dispatcher1 = Dispatcher::new(repo, configs);

    let dispatcher2 = dispatcher1.clone();

    assert_eq!(dispatcher1.table_count(), dispatcher2.table_count());
    assert!(dispatcher1.has_table("users"));
    assert!(dispatcher2.has_table("users"));

    let ctx1 = dispatcher1.get_table("users").await.unwrap();
    let _ = ctx1.insert(&crate::types::value::InnerValue::Int(42)).await.unwrap();

    let ctx2 = dispatcher2.get_table("users").await.unwrap();
    assert_eq!(ctx2.count().await.unwrap(), 1);
}
