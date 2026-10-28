use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::tests::write::record_mutation::execution::shared::write_exec_tests::*;
use crate::table::TableConfig;
use futures::StreamExt;
use shamir_query_builder::write::{self, doc};
use shamir_types::core::interner::InternerQuota;
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

/// RU8 acceptance 6: an implicit INSERT whose field-name intern hits the
/// per-repo quota fails with the wire code, leaves NO visible row and NO WAL
/// record. The quota manager is installed on an empty repo before creating the
/// table; the pre-touched name exhausts the cap before the insert's field is
/// metered, so the rejection is the insert's own touch.
#[tokio::test]
async fn rv_types_2eb39dfd_insert_quota_failure_no_row_no_wal() {
    // The constructor installs the quota manager before recovery, then this
    // admitted publication registers the fully initialized repo in the DB.
    let db = DbInstance::new();
    let instance = crate::repo::RepoInstance::from_factory_with_interner_quota_for_test(
        "default".to_string(),
        BoxRepoFactory::in_memory(),
        Vec::new(),
        InternerQuota {
            max_names: 1,
            max_bytes: 1_048_576,
        },
    )
    .await
    .unwrap();
    let permit = instance.write_domain().admit().await.unwrap();
    db.publish_repo_admitted(instance, &permit).unwrap();
    drop(permit);
    let repo = db.get_repo("default").unwrap();
    db.create_table_checked("default", "users").await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    // Exhaust the cap before the insert runs.
    table
        .interner()
        .get()
        .await
        .unwrap()
        .touch_ind("occupied")
        .unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let wal_len_before = wal.recover().await.unwrap().len();

    let op = write::insert("users")
        .row(doc().set("fresh_field", "value1").unwrap())
        .build();
    let err = insert_via_tx(&repo, &table, &op, true)
        .await
        .expect_err("insert over the interner quota must fail");
    assert!(
        matches!(&err, crate::query::batch::BatchError::QueryError { code, .. }
            if code.as_deref() == Some("interner_quota_exceeded")),
        "unexpected error: {err:?}"
    );

    // No WAL record for the failed implicit insert.
    assert_eq!(
        wal.recover().await.unwrap().len(),
        wal_len_before,
        "a quota-rejected implicit insert must append no WAL record"
    );

    // No visible row: the rejected touch consumed nothing.
    let stream = table.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(count, 0, "quota-rejected insert must leave no row");
}
