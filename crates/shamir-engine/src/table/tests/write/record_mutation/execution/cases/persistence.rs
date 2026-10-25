use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::tests::write::record_mutation::execution::shared::write_exec_tests::*;
use crate::table::TableConfig;
use shamir_query_builder::write::{self, doc};
use shamir_types::types::common::new_map;

/// This test verifies that new interned keys are persisted after insert.
/// Without auto-persist, a "restart" (new InternerManager on same storage)
/// would lose the keys and fail to read back the data correctly.
#[tokio::test]
async fn test_interner_persisted_after_insert() {
    // Setup: create table via raw storage (so we can simulate restart)
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    let repo = db.get_repo("default").unwrap();

    // Insert records with new field keys ("brand_new_field" never seen before)
    let op = write::insert("users")
        .row(doc().set("brand_new_field", "value1").unwrap())
        .row(doc().set("brand_new_field", "value2").unwrap())
        .build();
    insert_via_tx(&repo, &table, &op, true).await.unwrap();

    // Verify the key was interned
    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("brand_new_field").is_some());

    // Simulate "restart": create a new InternerManager on the same storage
    // The key test: do the persisted entries contain "brand_new_field"?
    let entries = interner.all_entries();
    assert!(
        entries
            .iter()
            .any(|(_, user_key)| user_key.as_str() == "brand_new_field"),
        "brand_new_field should be in interner entries after persist"
    );
}

/// Same test for execute_update: new set fields should be persisted.
#[tokio::test]
async fn test_interner_persisted_after_update() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Update with a brand new field key
    let op = write::update("users")
        .set(doc().set("completely_new_key", 42_i64).unwrap())
        .build()
        .unwrap();
    update_via_tx(&repo, &table, &op, &refs).await.unwrap();

    // The new key should be persisted
    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("completely_new_key").is_some());
    let entries = interner.all_entries();
    assert!(
        entries
            .iter()
            .any(|(_, uk)| uk.as_str() == "completely_new_key"),
        "completely_new_key should be persisted"
    );
}

/// Same test for execute_set: upsert should persist new keys.
#[tokio::test]
async fn test_interner_persisted_after_set() {
    let (table, repo) = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("unique_field_xyz", "val").unwrap())
        .value(
            doc()
                .set("unique_field_xyz", "val")
                .unwrap()
                .set("another_new_field", 99_i64)
                .unwrap(),
        )
        .build()
        .unwrap();
    set_via_tx(&repo, &table, &op).await.unwrap();

    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("unique_field_xyz").is_some());
    assert!(interner.get_ind("another_new_field").is_some());
}
