use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

fn create_test_instance() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new(
        "test".into(),
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

// ============================================================================
// III.1 — table_by_token O(1) resolution
// ============================================================================

#[tokio::test]
async fn table_by_token_is_constant_time_and_correct() {
    // Register many tables, both via the constructor and via add_table,
    // then assert every one resolves through the reverse index to the
    // correct TableManager, and an unknown token returns None cleanly.
    let repo = Arc::new(InMemoryRepo::new());
    let initial: Vec<TableConfig> = (0..50)
        .map(|i| TableConfig::new(format!("tbl_init_{i}")))
        .collect();
    let instance = RepoInstance::new("tt".into(), BoxRepo::InMemory(repo), initial);

    // A batch added dynamically after construction.
    for i in 0..50 {
        instance.add_table(TableConfig::new(format!("tbl_dyn_{i}")));
    }

    // Every registered name resolves by its deterministic token to itself.
    for i in 0..50 {
        for prefix in ["tbl_init_", "tbl_dyn_"] {
            let name = format!("{prefix}{i}");
            let token = table_token_for(&name);
            let resolved = instance
                .table_by_token(token)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("token for '{name}' did not resolve"));
            assert_eq!(resolved.name(), name);
        }
    }

    // A token that no table owns resolves to None (not an error, no panic).
    let bogus = table_token_for("table_that_was_never_registered");
    assert!(instance.table_by_token(bogus).await.unwrap().is_none());
}

#[tokio::test]
async fn table_by_token_drops_with_remove_table() {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new(
        "tt".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("alpha"), TableConfig::new("beta")],
    );

    let alpha_token = table_token_for("alpha");
    assert!(instance
        .table_by_token(alpha_token)
        .await
        .unwrap()
        .is_some());

    // After removing the config, the token must no longer resolve — a
    // stale reverse-index entry must not resurrect a dropped table.
    assert!(instance.remove_table("alpha"));
    assert!(instance
        .table_by_token(alpha_token)
        .await
        .unwrap()
        .is_none());

    // The sibling table is unaffected.
    let beta_token = table_token_for("beta");
    let beta = instance.table_by_token(beta_token).await.unwrap().unwrap();
    assert_eq!(beta.name(), "beta");
}

#[tokio::test]
async fn add_table_twice_is_idempotent_for_token_lookup() {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new("tt".into(), BoxRepo::InMemory(repo), vec![]);

    instance.add_table(TableConfig::new("dup"));
    instance.add_table(TableConfig::new("dup"));

    let token = table_token_for("dup");
    let resolved = instance.table_by_token(token).await.unwrap().unwrap();
    assert_eq!(resolved.name(), "dup");
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
