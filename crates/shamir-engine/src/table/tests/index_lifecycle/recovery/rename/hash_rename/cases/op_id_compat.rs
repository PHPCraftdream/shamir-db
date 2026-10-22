use crate::index::index_manager::HashRenameTombstone;
use crate::table::TableManager;
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

use crate::table::tests::index_lifecycle::recovery::rename::hash_rename::shared::p997_hash_rename_durability_tests::*;

// ============================================================================
// #1048: backward-compatibility test — a tombstone with op_id: None (pre-#1048
// data) recovers cleanly without attempting an op-status write or erroring.
// ============================================================================

#[tokio::test]
async fn p1048_backward_compat_none_op_id_recovers_cleanly() {
    let (data_store, info_store) = make_stores();

    // Step 1: create a table with a regular index and data
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        let email_field = key_id(&mgr, "email").await;
        mgr.insert(&record_with_str(email_field, "a@b.com"))
            .await
            .unwrap();
        mgr.interner.persist().await.unwrap();
        mgr.create_index("by_email", &["email"]).await.unwrap();
        drop(mgr);
    }

    // Step 2: seed a tombstone with op_id: None (simulating pre-#1048 data)
    let tombstone = HashRenameTombstone {
        old_name: "by_email".to_string(),
        new_name: "by_email_new".to_string(),
        paths: vec!["email".to_string()],
        op_id: None, // ← pre-#1048 tombstone (no op_id)
    };
    seed_regular_rename_tombstone(&info_store, &tombstone).await;

    // Step 3: reopen — recovery must complete the rename without error
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .expect("recovery must succeed even with None op_id");

    // Verify the rename completed
    assert!(!mgr.index_exists("by_email").await);
    assert!(mgr.index_exists("by_email_new").await);

    // Tombstone must be cleared
    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared after recovery"
    );

    // No op-status record should have been written (since op_id was None)
    use crate::table::ddl_op_log;
    use shamir_query_types::read::DdlOpStatus;
    let dummy_op_id = RecordId::system("dummy");
    let status: Option<DdlOpStatus> = ddl_op_log::read_op_status(&info_store, &dummy_op_id)
        .await
        .expect("read_op_status should not error");
    assert!(
        status.is_none(),
        "no op-status record should exist for a None op_id tombstone"
    );
}
