use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

fn create_test_instance() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new(
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("users"), TableConfig::new("orders")],
    )
}

#[tokio::test]
async fn test_repo_instance_creation() {
    let instance = create_test_instance();

    assert_eq!(instance.table_count(), 2);
    assert!(instance.has_table("users"));
    assert!(instance.has_table("orders"));
    assert!(!instance.has_table("products"));
}

#[tokio::test]
async fn test_list_table_names() {
    let instance = create_test_instance();
    let names = instance.list_table_names();

    assert_eq!(names.len(), 2);
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"orders".to_string()));
}

#[tokio::test]
async fn test_get_table_lazy() {
    let instance = create_test_instance();

    let table1 = instance.get_table("users").await.unwrap();
    assert_eq!(table1.name(), "users");

    let table2 = instance.get_table("users").await.unwrap();
    assert_eq!(table2.name(), "users");
}

#[tokio::test]
async fn test_get_table_not_configured() {
    let instance = create_test_instance();

    let result = instance.get_table("products").await;
    assert!(result.is_err());
}

// ============================================================================
// Index API tests
// ============================================================================

#[tokio::test]
async fn test_repo_instance_create_index() {
    let instance = create_test_instance();

    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();

    assert!(instance.index_exists("users", "email_idx").await.unwrap());
    assert!(!instance.index_exists("users", "nonexistent").await.unwrap());
}

#[tokio::test]
async fn test_repo_instance_create_composite_index() {
    let instance = create_test_instance();

    instance
        .create_index("users", "name_city_idx", &["name", "city"])
        .await
        .unwrap();

    assert!(instance
        .index_exists("users", "name_city_idx")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_repo_instance_create_nested_path_index() {
    let instance = create_test_instance();

    instance
        .create_index("users", "city_idx", &["address.city"])
        .await
        .unwrap();

    assert!(instance.index_exists("users", "city_idx").await.unwrap());
}

#[tokio::test]
async fn test_repo_instance_create_unique_index() {
    let instance = create_test_instance();

    instance
        .create_unique_index("users", "email_unique", &["email"])
        .await
        .unwrap();

    // Unique index exists in unique collection, not regular
    assert!(!instance
        .index_exists("users", "email_unique")
        .await
        .unwrap());
    assert!(instance
        .unique_index_exists("users", "email_unique")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_repo_instance_drop_index() {
    let instance = create_test_instance();

    // Create index
    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();
    assert!(instance.index_exists("users", "email_idx").await.unwrap());

    // Drop index
    let dropped = instance.drop_index("users", "email_idx").await.unwrap();
    assert!(dropped);
    assert!(!instance.index_exists("users", "email_idx").await.unwrap());

    // Drop again returns false
    let dropped_again = instance.drop_index("users", "email_idx").await.unwrap();
    assert!(!dropped_again);
}

#[tokio::test]
async fn test_repo_instance_drop_unique_index() {
    let instance = create_test_instance();

    // Create unique index
    instance
        .create_unique_index("users", "email_unique", &["email"])
        .await
        .unwrap();
    assert!(instance
        .unique_index_exists("users", "email_unique")
        .await
        .unwrap());

    // Drop unique index
    let dropped = instance
        .drop_unique_index("users", "email_unique")
        .await
        .unwrap();
    assert!(dropped);
    assert!(!instance
        .unique_index_exists("users", "email_unique")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_repo_instance_lookup_by_index() {
    let instance = create_test_instance();

    // Create index
    instance
        .create_index("users", "status_idx", &["status"])
        .await
        .unwrap();

    // Lookup with no data returns empty
    let results = instance
        .lookup_by_index(
            "users",
            "status_idx",
            &[InnerValue::Str("active".to_string())],
        )
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_repo_instance_index_isolation_between_tables() {
    let instance = create_test_instance();

    // Create index on users table
    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();

    // Create different index on orders table
    instance
        .create_index("orders", "user_id_idx", &["user_id"])
        .await
        .unwrap();

    // Check isolation
    assert!(instance.index_exists("users", "email_idx").await.unwrap());
    assert!(!instance.index_exists("users", "user_id_idx").await.unwrap());
    assert!(!instance.index_exists("orders", "email_idx").await.unwrap());
    assert!(instance
        .index_exists("orders", "user_id_idx")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_repo_instance_clone_shares_state() {
    let instance = create_test_instance();
    let instance2 = instance.clone();

    // Create index through first instance
    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();

    // Check visible through second instance
    assert!(instance2.index_exists("users", "email_idx").await.unwrap());
}

#[tokio::test]
async fn test_repo_instance_crud_with_table() {
    let instance = create_test_instance();

    let table = instance.get_table("users").await.unwrap();

    // Insert
    let value = InnerValue::Str("test@example.com".to_string());
    let id = table.insert(&value).await.unwrap();
    assert_eq!(table.count().await.unwrap(), 1);

    // Get
    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, value);

    // Delete
    let deleted = table.delete(id).await.unwrap();
    assert!(deleted);
    assert_eq!(table.count().await.unwrap(), 0);
}
