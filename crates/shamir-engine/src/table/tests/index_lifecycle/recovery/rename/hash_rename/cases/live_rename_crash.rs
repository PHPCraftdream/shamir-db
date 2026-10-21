use std::sync::Arc;
use shamir_types::types::value::InnerValue;
use crate::table::TableManager;
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;


use crate::table::tests::index_lifecycle::recovery::rename::hash_rename::fixtures::p997_hash_rename_durability_tests::*;

// ============================================================================
// Live rename crash simulation via pause hook
// ============================================================================

#[tokio::test]
async fn p997_regular_live_rename_crash_at_mid_hook() {
    let (data_store, info_store) = make_stores();
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

    // Install the mid-rename pause hook.
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.index_manager_ref()
        .set_rename_mid_pause_hook(Some(Arc::clone(&hook)));

    // Start rename — it will park at the hook (tombstone written, new created,
    // old NOT yet dropped).
    let mgr_c = mgr.clone();
    tokio::select! {
        _ = mgr_c.rename_index("by_email", "by_email_new", None) => {
            panic!("rename completed before mid-pause hook fired");
        }
        _ = hook.wait_until_parked() => {
            // Parked: mid-rename.
        }
    }

    // Simulate crash: drop the manager (cancels the rename future).
    drop(mgr_c);
    drop(mgr);

    // Reopen — recovery MUST finish the rename.
    let new_mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !new_mgr.index_exists("by_email").await,
        "old index must be gone after recovery"
    );
    assert!(
        new_mgr.index_exists("by_email_new").await,
        "new index must exist after recovery"
    );

    let results = new_mgr
        .lookup_by_index("by_email_new", &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "postings must be resolvable under new name after live crash recovery"
    );

    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone cleared after live crash recovery"
    );
}

#[tokio::test]
async fn p997_unique_live_rename_crash_at_mid_hook() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let email_field = key_id(&mgr, "email").await;
    mgr.insert(&record_with_str(email_field, "unique@x.com"))
        .await
        .unwrap();
    mgr.interner.persist().await.unwrap();
    mgr.create_unique_index("uniq_email", &["email"])
        .await
        .unwrap();

    // Install the mid-rename pause hook.
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.index_manager_ref()
        .set_rename_mid_pause_hook(Some(Arc::clone(&hook)));

    // Start rename — it will park at the hook (tombstone written, old dropped,
    // new NOT yet created — the SEVERE crash window).
    let mgr_c = mgr.clone();
    tokio::select! {
        _ = mgr_c.rename_index("uniq_email", "uniq_email_new", None) => {
            panic!("rename completed before mid-pause hook fired");
        }
        _ = hook.wait_until_parked() => {
            // Parked: mid-rename (SEVERE window: neither index exists).
        }
    }

    // Simulate crash.
    drop(mgr_c);
    drop(mgr);

    // Reopen — recovery MUST rebuild the unique constraint from the tombstone.
    let new_mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !new_mgr.unique_index_exists("uniq_email").await,
        "old unique index must be gone"
    );
    assert!(
        new_mgr.unique_index_exists("uniq_email_new").await,
        "new unique index must be REBUILT after SEVERE crash"
    );

    // Uniqueness must be enforced.
    let email_field = key_id(&new_mgr, "email").await;
    let dup = new_mgr
        .insert(&record_with_str(email_field, "unique@x.com"))
        .await;
    assert!(
        dup.is_err(),
        "duplicate must be rejected — unique constraint was rebuilt by recovery"
    );

    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone cleared after SEVERE crash recovery"
    );
}

// ============================================================================
// Regression smoke: normal (no-crash) rename still works
// ============================================================================

#[tokio::test]
async fn p997_regular_rename_no_crash_regression() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let email_field = key_id(&mgr, "email").await;
    let rid = mgr
        .insert(&record_with_str(email_field, "a@b.com"))
        .await
        .unwrap();
    mgr.interner.persist().await.unwrap();
    mgr.create_index("by_email", &["email"]).await.unwrap();

    // Normal rename — no crash.
    mgr.rename_index("by_email", "by_email_new", None)
        .await
        .unwrap();

    assert!(!mgr.index_exists("by_email").await);
    assert!(mgr.index_exists("by_email_new").await);

    let results = mgr
        .lookup_by_index("by_email_new", &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(results.contains(&rid), "record must survive normal rename");

    // Tombstone must be cleared after a successful rename.
    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared after successful rename"
    );
}

#[tokio::test]
async fn p997_unique_rename_no_crash_regression() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let email_field = key_id(&mgr, "email").await;
    let rid = mgr
        .insert(&record_with_str(email_field, "unique@x.com"))
        .await
        .unwrap();
    mgr.interner.persist().await.unwrap();
    mgr.create_unique_index("uniq_email", &["email"])
        .await
        .unwrap();

    // Normal rename — no crash.
    mgr.rename_index("uniq_email", "uniq_email_new", None)
        .await
        .unwrap();

    assert!(!mgr.unique_index_exists("uniq_email").await);
    assert!(mgr.unique_index_exists("uniq_email_new").await);

    let new_id = key_id(&mgr, "uniq_email_new").await;
    let owner = mgr
        .index_manager_ref()
        .lookup_by_unique_index(new_id, &[InnerValue::Str("unique@x.com".into())])
        .await
        .unwrap();
    assert_eq!(owner, Some(rid), "unique record must survive rename");

    // Uniqueness must be enforced.
    let dup = mgr
        .insert(&record_with_str(email_field, "unique@x.com"))
        .await;
    assert!(dup.is_err(), "duplicate must be rejected after rename");

    // Tombstone must be cleared.
    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared after successful rename"
    );
}
