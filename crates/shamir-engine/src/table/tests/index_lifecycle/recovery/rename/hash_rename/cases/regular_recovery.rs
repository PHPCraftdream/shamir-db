use std::sync::Arc;
use shamir_types::types::value::InnerValue;
use crate::table::TableManager;


use crate::table::tests::index_lifecycle::recovery::rename::hash_rename::fixtures::p997_hash_rename_durability_tests::*;

// Matrix row 1: tombstone present, old present, new absent
// → crash before create_index ran. Recovery: create new, drop old, clear.
#[tokio::test]
async fn p997_regular_crash_before_create() {
    let (data_store, info_store) =
        seed_table_with_regular_index("by_email", &["a@b.com", "c@d.com"]).await;

    // Seed: tombstone written, nothing mutated.
    seed_regular_rename_tombstone(&info_store, &rename_entry("by_email", "by_email_new")).await;

    // Reopen — recovery MUST complete the rename.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr.index_exists("by_email").await,
        "old index must be gone after recovery"
    );
    assert!(
        mgr.index_exists("by_email_new").await,
        "new index must exist after recovery"
    );

    // Postings must be resolvable under the new name.
    let results = mgr
        .lookup_by_index("by_email_new", &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "postings must be resolvable under new name"
    );

    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared"
    );
}

// Matrix row 2: tombstone present, old present, new present (Ready)
// → crash between create and drop. Recovery: drop old, clear.
#[tokio::test]
async fn p997_regular_crash_between_create_and_drop() {
    let (data_store, info_store) =
        seed_table_with_regular_index("by_email", &["a@b.com", "c@d.com"]).await;

    // Simulate: create the new index (under a fresh manager), then seed the
    // tombstone to simulate the crash state.
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        // No tombstone on disk yet (IndexManager::new didn't find one).
        // Create the new index (simulating create_index succeeding).
        mgr.create_index("by_email_new", &["email"]).await.unwrap();
        drop(mgr);
    }

    // Now seed the tombstone — both old and new exist.
    seed_regular_rename_tombstone(&info_store, &rename_entry("by_email", "by_email_new")).await;

    // Reopen — recovery MUST drop old and keep new.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr.index_exists("by_email").await,
        "old index must be dropped by recovery"
    );
    assert!(
        mgr.index_exists("by_email_new").await,
        "new index must survive recovery"
    );

    let results = mgr
        .lookup_by_index("by_email_new", &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "postings must be resolvable under new name"
    );

    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared"
    );
}

// Matrix row 3: tombstone present, old absent, new present (Ready)
// → crash after drop, before clear. Recovery: clear (no-op otherwise).
#[tokio::test]
async fn p997_regular_crash_after_drop_before_clear() {
    let (data_store, info_store) =
        seed_table_with_regular_index("by_email", &["a@b.com", "c@d.com"]).await;

    // Simulate: create new, drop old (under a fresh manager), then seed tombstone.
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        mgr.create_index("by_email_new", &["email"]).await.unwrap();
        mgr.drop_index("by_email", None).await.unwrap();
        drop(mgr);
    }

    seed_regular_rename_tombstone(&info_store, &rename_entry("by_email", "by_email_new")).await;

    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr.index_exists("by_email").await,
        "old index must stay gone"
    );
    assert!(
        mgr.index_exists("by_email_new").await,
        "new index must stay present"
    );

    let results = mgr
        .lookup_by_index("by_email_new", &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(!results.is_empty(), "postings must be resolvable");

    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared"
    );
}

// ============================================================================
// REGULAR: idempotence — double restart is a no-op
// ============================================================================

#[tokio::test]
async fn p997_regular_recovery_idempotent_double_restart() {
    let (data_store, info_store) =
        seed_table_with_regular_index("by_email", &["a@b.com", "c@d.com"]).await;

    seed_regular_rename_tombstone(&info_store, &rename_entry("by_email", "by_email_new")).await;

    // First restart — recovery completes the rename.
    let mgr1 = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    assert!(mgr1.index_exists("by_email_new").await);
    assert!(!mgr1.index_exists("by_email").await);
    drop(mgr1);

    // Second restart — must be a clean no-op.
    let mgr2 = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(
        mgr2.index_exists("by_email_new").await,
        "new index still present after second restart"
    );
    assert!(
        !mgr2.index_exists("by_email").await,
        "old index still gone after second restart"
    );
    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone empty after second restart"
    );
}

// ============================================================================
// UNIQUE: crash-state matrix
// ============================================================================
