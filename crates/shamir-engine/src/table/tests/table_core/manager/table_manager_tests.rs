use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Repo;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[tokio::test]
async fn cached_token_preserves_hash_across_construction_clone_and_reopen() {
    let repo = InMemoryRepo::new();
    let data = repo.store_get("__data__token".to_string()).await.unwrap();
    let info = repo.store_get("__info__token".to_string()).await.unwrap();
    let name = "token";
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let expected = hasher.finish();
    let mut table = TableManager::create(name.to_string(), data.clone(), info.clone())
        .await
        .unwrap();
    assert_eq!(table.table_token(), expected);
    assert_eq!(table.clone().table_token(), expected);
    let reopened = TableManager::create(name.to_string(), data.clone(), info.clone())
        .await
        .unwrap();
    assert_eq!(reopened.table_token(), expected);
    let domain = crate::repo::RepoWriteDomain::new("repo".to_string());
    let permit = domain.admit().await.unwrap();
    let admitted = TableManager::create_with_domain_admitted(
        name.to_string(),
        data,
        info,
        Arc::clone(&domain),
        &permit,
    )
    .await
    .unwrap();
    assert_eq!(admitted.table_token(), expected);
    // Test-only mutation discriminates a cached token from rehashing the name.
    table.name = "different".to_string();
    assert_eq!(table.table_token(), expected);
    assert_eq!(table.clone().table_token(), expected);
}

#[tokio::test]
async fn test_table_manager_creation() {
    let repo = Arc::new(InMemoryRepo::new());
    let data_store = repo.store_get("__data__test".to_string()).await.unwrap();
    let info_store = repo.store_get("__info__test".to_string()).await.unwrap();

    let data_store: Arc<dyn shamir_storage::types::Store> = data_store;
    let info_store: Arc<dyn shamir_storage::types::Store> = info_store;

    use crate::index::index_manager::IndexManager;
    use crate::table::interner_manager::InternerManager;
    use crate::table::record_counter::RecordCounter;

    let interner = InternerManager::new(Arc::clone(&info_store));
    let counter = Arc::new(RecordCounter::new(Arc::clone(&info_store)));
    let index_manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    use crate::table::Table;
    let table = Table::new(Arc::clone(&data_store));

    let ctx = TableManager::new("test".to_string(), table, interner, counter, index_manager);
    assert_eq!(ctx.name(), "test");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "test".hash(&mut hasher);
    assert_eq!(ctx.table_token(), hasher.finish());
}

#[tokio::test]
async fn test_table_manager_clone() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let ctx1 = db.get_table("default", "users").await.unwrap();
    let ctx2 = ctx1.clone();

    assert_eq!(ctx1.name(), ctx2.name());
    assert!(std::ptr::eq(ctx1.table_for_test(), ctx2.table_for_test()));
}

#[tokio::test]
async fn test_table_manager_components() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let ctx = db.get_table("default", "users").await.unwrap();

    assert_eq!(ctx.name(), "users");

    let value = InnerValue::Str("test".to_string());
    let record_id = ctx.insert(&value).await.unwrap();
    assert_eq!(ctx.count().await.unwrap(), 1);

    let retrieved = ctx.get(record_id).await.unwrap();
    assert_eq!(retrieved, value);
}

#[tokio::test]
async fn test_table_manager_insert_many_no_index() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let ctx = db.get_table("default", "users").await.unwrap();

    let values: Vec<InnerValue> = (0..7)
        .map(|i| InnerValue::Str(format!("rec-{}", i)))
        .collect();
    let ids = ctx.insert_many(&values).await.unwrap();
    assert_eq!(ids.len(), 7);

    // Counter reflects the batch as a whole.
    assert_eq!(ctx.count().await.unwrap(), 7);

    // Each id resolves to its corresponding value in input order.
    for (id, expected) in ids.iter().zip(values.iter()) {
        let got = ctx.get(*id).await.unwrap();
        assert_eq!(&got, expected);
    }

    // Empty input does not touch the counter and returns empty.
    let empty = ctx.insert_many(&[]).await.unwrap();
    assert!(empty.is_empty());
    assert_eq!(ctx.count().await.unwrap(), 7);
}

#[tokio::test]
async fn test_table_manager_insert_many_with_unique_index() {
    use shamir_types::types::common::new_map;

    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let ctx = db.get_table("default", "users").await.unwrap();

    // Set up a UNIQUE index on `id` via the string-paths API.
    ctx.create_unique_index("by_id", &["id"]).await.unwrap();

    // Build records using the interner so the index path resolves.
    let interner = ctx.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let mk = |id: &str| -> InnerValue {
        let mut m = new_map();
        m.insert(id_key.clone(), InnerValue::Str(id.to_string()));
        InnerValue::Map(m)
    };

    // Happy path — three distinct ids.
    let ok = vec![mk("u1"), mk("u2"), mk("u3")];
    let ids = ctx.insert_many(&ok).await.unwrap();
    assert_eq!(ids.len(), 3);
    assert_eq!(ctx.count().await.unwrap(), 3);

    // Duplicate id in the second slot — the second
    // `validate_unique_for_create` call sees the prior write and
    // rejects the whole batch.
    let bad = vec![mk("u4"), mk("u1")];
    let res = ctx.insert_many(&bad).await;
    assert!(res.is_err(), "duplicate unique key must reject the batch");
    // Counter must not have advanced past the rejected batch.
    assert_eq!(ctx.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_insert_many_rejects_within_batch_duplicate_unique() {
    // Regression test: two records in the SAME batch carrying the
    // same value for a unique index must reject the batch. Per-row
    // `validate_unique_for_create` only checks already-persisted
    // state and cannot see the prior element of the same batch on
    // its own.
    use shamir_types::types::common::new_map;

    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let ctx = db.get_table("default", "users").await.unwrap();
    ctx.create_unique_index("by_id", &["id"]).await.unwrap();

    let interner = ctx.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let mk = |id: &str| -> InnerValue {
        let mut m = new_map();
        m.insert(id_key.clone(), InnerValue::Str(id.to_string()));
        InnerValue::Map(m)
    };

    // Two distinct records with the SAME unique value in one batch.
    let batch = vec![mk("dup"), mk("dup")];
    let res = ctx.insert_many(&batch).await;
    assert!(
        res.is_err(),
        "duplicate unique key within one batch must reject the batch"
    );
    // Neither record landed — counter still zero, no orphans.
    assert_eq!(
        ctx.count().await.unwrap(),
        0,
        "counter must not have advanced when within-batch unique check rejected the batch"
    );
}

// ============================================================================
// Index API tests (string paths)
// ============================================================================

#[tokio::test]
async fn test_create_index_simple() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let table = db.get_table("default", "users").await.unwrap();

    // Create index with string path
    table.create_index("email_idx", &["email"]).await.unwrap();

    // Check index exists
    assert!(table.index_exists("email_idx").await);
    assert!(!table.index_exists("nonexistent").await);
}

#[tokio::test]
async fn test_create_index_composite() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let table = db.get_table("default", "users").await.unwrap();

    // Create composite index
    table
        .create_index("name_city_idx", &["name", "city"])
        .await
        .unwrap();

    assert!(table.index_exists("name_city_idx").await);
}

#[tokio::test]
async fn test_create_index_nested_path() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let table = db.get_table("default", "users").await.unwrap();

    // Create index with nested path
    table
        .create_index("city_idx", &["address.city"])
        .await
        .unwrap();

    assert!(table.index_exists("city_idx").await);
}

#[tokio::test]
async fn test_drop_index() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let table = db.get_table("default", "users").await.unwrap();

    // Create and drop
    table.create_index("email_idx", &["email"]).await.unwrap();
    assert!(table.index_exists("email_idx").await);

    let dropped = table.drop_index("email_idx", None).await.unwrap();
    assert!(dropped);
    assert!(!table.index_exists("email_idx").await);

    // Drop non-existent returns false
    let dropped_again = table.drop_index("email_idx", None).await.unwrap();
    assert!(!dropped_again);
}

#[tokio::test]
async fn test_unique_index() {
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let table = db.get_table("default", "users").await.unwrap();

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
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let table = db.get_table("default", "users").await.unwrap();

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
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let table = db.get_table("default", "users").await.unwrap();

    // Insert and retrieve
    let value = InnerValue::Str("hello".to_string());
    let id = table.insert(&value).await.unwrap();

    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, value);
}

#[tokio::test]
async fn rv_funclib_5fa455f7_batch_dup_canonical_key() {
    // Regression (RV-FUNCLIB-5FA455F7): the batch-local `batch_seen`
    // duplicate map must key rows by the CANONICAL index key, not
    // bincode-serialized leaves — F64(1.0) and Int(1) are one unique
    // value; NaN never conflicts with NaN.
    let configs = vec![TableConfig::new("users")];
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: configs,
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let ctx = db.get_table("default", "users").await.unwrap();
    ctx.create_unique_index("by_id", &["id"]).await.unwrap();

    let interner = ctx.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let mk = |n: InnerValue| -> InnerValue {
        let mut m = new_map();
        m.insert(id_key.clone(), n);
        InnerValue::Map(m)
    };

    // F64(1.0) then Int(1) in one batch: same canonical unique value —
    // must be rejected by the batch-seen catch.
    let canonical_dup = vec![mk(InnerValue::F64(1.0)), mk(InnerValue::Int(1))];
    let res = ctx.insert_many(&canonical_dup).await;
    assert!(
        matches!(res, Err(shamir_storage::error::DbError::DuplicateKey(_))),
        "F64(1.0) + Int(1) in one batch must be a unique conflict"
    );
    assert_eq!(
        ctx.count().await.unwrap(),
        0,
        "rejected batch must not land any rows"
    );

    // Two NaN rows in one batch: NaN is not a conflict — must succeed.
    let nan_batch = vec![mk(InnerValue::F64(f64::NAN)), mk(InnerValue::F64(f64::NAN))];
    let ids = ctx.insert_many(&nan_batch).await.unwrap();
    assert_eq!(ids.len(), 2, "NaN rows must not false-conflict");
    assert_eq!(ctx.count().await.unwrap(), 2);
}
