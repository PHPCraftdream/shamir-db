use std::sync::Arc;
use shamir_types::types::value::InnerValue;
use crate::table::TableManager;


use crate::table::tests::index_lifecycle::recovery::rename::hash_rename::fixtures::p997_hash_rename_durability_tests::*;

// Matrix row 1: tombstone present, old present, new absent
// → crash before drop ran. Recovery: create new, drop old, clear.
#[tokio::test]
async fn p997_unique_crash_before_drop() {
    let (data_store, info_store) =
        seed_table_with_unique_index("uniq_email", &["a@b.com", "c@d.com"]).await;

    seed_unique_rename_tombstone(&info_store, &rename_entry("uniq_email", "uniq_email_new")).await;

    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr.unique_index_exists("uniq_email").await,
        "old unique index must be gone after recovery"
    );
    assert!(
        mgr.unique_index_exists("uniq_email_new").await,
        "new unique index must exist after recovery"
    );

    // Uniqueness must be enforced under the new name.
    let dup = mgr
        .insert(&record_with_str(key_id(&mgr, "email").await, "a@b.com"))
        .await;
    assert!(
        dup.is_err(),
        "duplicate insert must be rejected after recovery"
    );

    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared"
    );
}

// Matrix row 2 (SEVERE): tombstone present, both old and new absent
// → crash after drop, before create. The unique constraint is SILENTLY GONE.
// Recovery MUST rebuild from the tombstone's stored paths.
#[tokio::test]
async fn p997_unique_severe_both_absent() {
    let (data_store, info_store) =
        seed_table_with_unique_index("uniq_email", &["a@b.com", "c@d.com"]).await;

    // Simulate: drop the old unique index (under a fresh manager), then seed tombstone.
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        mgr.drop_unique_index("uniq_email", None).await.unwrap();
        drop(mgr);
    }

    seed_unique_rename_tombstone(&info_store, &rename_entry("uniq_email", "uniq_email_new")).await;

    // Reopen — recovery MUST rebuild the unique constraint from the tombstone.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr.unique_index_exists("uniq_email").await,
        "old unique index must be gone"
    );
    assert!(
        mgr.unique_index_exists("uniq_email_new").await,
        "new unique index must be REBUILT by recovery"
    );

    // CRITICAL: uniqueness must be enforced.
    let dup = mgr
        .insert(&record_with_str(key_id(&mgr, "email").await, "a@b.com"))
        .await;
    assert!(
        dup.is_err(),
        "duplicate must be rejected — the unique constraint was REBUILT by recovery"
    );

    // Existing data must be findable via the new unique index.
    let new_id = key_id(&mgr, "uniq_email_new").await;
    let owner = mgr
        .index_manager_ref()
        .lookup_by_unique_index(new_id, &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(owner.is_some(), "unique lookup must find the record");

    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared"
    );
}

// Matrix row 3: tombstone present, old absent, new present (Ready)
// → crash after create, before clear. Recovery: clear.
#[tokio::test]
async fn p997_unique_crash_after_create_before_clear() {
    let (data_store, info_store) =
        seed_table_with_unique_index("uniq_email", &["a@b.com", "c@d.com"]).await;

    // Simulate: drop old + create new (under a fresh manager), then seed tombstone.
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        // Use rename_index directly (which does the full sequence), then
        // re-seed the tombstone to simulate the crash-before-clear state.
        mgr.rename_index("uniq_email", "uniq_email_new", None)
            .await
            .unwrap();
        drop(mgr);
    }

    seed_unique_rename_tombstone(&info_store, &rename_entry("uniq_email", "uniq_email_new")).await;

    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr.unique_index_exists("uniq_email").await,
        "old must stay gone"
    );
    assert!(
        mgr.unique_index_exists("uniq_email_new").await,
        "new must stay present"
    );

    let dup = mgr
        .insert(&record_with_str(key_id(&mgr, "email").await, "a@b.com"))
        .await;
    assert!(dup.is_err(), "uniqueness still enforced");

    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared"
    );
}

// ============================================================================
// UNIQUE: idempotence — double restart is a no-op
// ============================================================================

#[tokio::test]
async fn p997_unique_recovery_idempotent_double_restart() {
    let (data_store, info_store) =
        seed_table_with_unique_index("uniq_email", &["a@b.com", "c@d.com"]).await;

    seed_unique_rename_tombstone(&info_store, &rename_entry("uniq_email", "uniq_email_new")).await;

    let mgr1 = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    assert!(mgr1.unique_index_exists("uniq_email_new").await);
    assert!(!mgr1.unique_index_exists("uniq_email").await);
    drop(mgr1);

    let mgr2 = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(
        mgr2.unique_index_exists("uniq_email_new").await,
        "new unique index still present after second restart"
    );
    assert!(
        !mgr2.unique_index_exists("uniq_email").await,
        "old unique index still gone after second restart"
    );
    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone empty after second restart"
    );
}

// ============================================================================
// UNIQUE: duplicate-during-recovery hazard
// ============================================================================

/// The unique-duplicate hazard is theoretically impossible: recovery runs
/// during `TableManager::create`, before any writer can access the table.
/// This test verifies the invariant by pre-seeding duplicate data into the
/// store BEFORE recovery runs, then asserting that recovery fails the open
/// (does NOT silently accept duplicates).
#[tokio::test]
async fn p997_unique_duplicate_during_recovery_fails_open() {
    let (data_store, info_store) = seed_table_with_unique_index("uniq_email", &["a@b.com"]).await;

    // Simulate: drop the old unique index.
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        mgr.drop_unique_index("uniq_email", None).await.unwrap();
        drop(mgr);
    }

    // Now insert a DUPLICATE directly into the data store (bypassing the
    // unique constraint, which no longer exists). This simulates the
    // theoretical hazard: a write landing while the unique index was absent.
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        // No unique index exists, so this duplicate insert SUCCEEDS.
        let email_field = key_id(&mgr, "email").await;
        let _ = mgr
            .insert(&record_with_str(email_field, "a@b.com"))
            .await
            .unwrap();
        mgr.interner.persist().await.unwrap();
        drop(mgr);
    }

    // Seed the rename tombstone — recovery will try to rebuild the unique
    // index. The backfill MUST find the duplicate and FAIL the open.
    seed_unique_rename_tombstone(&info_store, &rename_entry("uniq_email", "uniq_email_new")).await;

    let result = TableManager::create("people".into(), data_store, Arc::clone(&info_store)).await;
    assert!(
        result.is_err(),
        "recovery MUST fail the open when a duplicate is found during the \
         unique-index rebuild — it must NOT silently accept duplicates"
    );
}
