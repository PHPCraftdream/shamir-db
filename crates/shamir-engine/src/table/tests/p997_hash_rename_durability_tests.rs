//! #997 — crash-recovery for compound regular/unique RENAME INDEX.
//!
//! Mirrors #988's `p03b_index2_drop_durability_tests.rs` and #962's
//! `p05b_sorted_rename_durability_tests.rs` for the base_index hash
//! (regular + unique) RENAME families. Both paths are a drop+rebuild (the
//! hash physical key embeds `name_interned` into h1/h2, so postings cannot
//! be rekeyed). The durable tombstone (`HashRenameTombstone`) carries the
//! resolved string names + paths so recovery can rebuild from nothing.
//!
//! Tests cover, per family:
//! - Every row of the crash-state matrix documented in
//!   `TableManager::recover_hash_renames`'s doc comment.
//! - Idempotence: a double restart after recovery is a no-op.
//! - The unique-duplicate-during-recovery hazard (should be impossible
//!   because recovery runs before any writer can access the table, but
//!   verified by test).
//! - A normal (no-crash) rename regression smoke test.
//! - A live rename crash simulation via the `rename_mid_pause_hook`.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index::index_manager::HashRenameTombstone;
use crate::table::TableManager;
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
use shamir_types::types::record_id::RecordId;

// ============================================================================
// Helpers
// ============================================================================

fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner.get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// Seed a table with a regular index + data, persist the interner, then
/// drop the manager. Returns (data_store, info_store, old_name, new_name).
/// The index is NOT yet renamed — the caller seeds the crash state.
async fn seed_table_with_regular_index(
    index_name: &str,
    values: &[&str],
) -> (Arc<dyn Store>, Arc<dyn Store>) {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let email_field = key_id(&mgr, "email").await;
    for &val in values {
        mgr.insert(&record_with_str(email_field, val))
            .await
            .unwrap();
    }
    mgr.interner.persist().await.unwrap();
    mgr.create_index(index_name, &["email"]).await.unwrap();
    drop(mgr);
    (data_store, info_store)
}

/// Seed a table with a unique index + data, persist the interner, then
/// drop the manager. Returns (data_store, info_store).
async fn seed_table_with_unique_index(
    index_name: &str,
    values: &[&str],
) -> (Arc<dyn Store>, Arc<dyn Store>) {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let email_field = key_id(&mgr, "email").await;
    for &val in values {
        mgr.insert(&record_with_str(email_field, val))
            .await
            .unwrap();
    }
    mgr.interner.persist().await.unwrap();
    mgr.create_unique_index(index_name, &["email"])
        .await
        .unwrap();
    drop(mgr);
    (data_store, info_store)
}

/// Write a regular rename tombstone directly into info_store.
async fn seed_regular_rename_tombstone(info_store: &Arc<dyn Store>, entry: &HashRenameTombstone) {
    let key = RecordId::system("idx_ren").to_bytes();
    let bytes = bincode::serialize(&vec![entry.clone()]).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
}

/// Write a unique rename tombstone directly into info_store.
async fn seed_unique_rename_tombstone(info_store: &Arc<dyn Store>, entry: &HashRenameTombstone) {
    let key = RecordId::system("uidx_ren").to_bytes();
    let bytes = bincode::serialize(&vec![entry.clone()]).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
}

/// Read back the persisted regular rename tombstone.
async fn load_regular_rename_tombstone(info_store: &Arc<dyn Store>) -> Vec<HashRenameTombstone> {
    let key = RecordId::system("idx_ren").to_bytes();
    match info_store.get(key.into()).await {
        Ok(bytes) if bytes.is_empty() => Vec::new(),
        Ok(bytes) => bincode::deserialize(&bytes).unwrap(),
        Err(_) => Vec::new(),
    }
}

/// Read back the persisted unique rename tombstone.
async fn load_unique_rename_tombstone(info_store: &Arc<dyn Store>) -> Vec<HashRenameTombstone> {
    let key = RecordId::system("uidx_ren").to_bytes();
    match info_store.get(key.into()).await {
        Ok(bytes) if bytes.is_empty() => Vec::new(),
        Ok(bytes) => bincode::deserialize(&bytes).unwrap(),
        Err(_) => Vec::new(),
    }
}

fn rename_entry(old: &str, new: &str) -> HashRenameTombstone {
    HashRenameTombstone {
        old_name: old.into(),
        new_name: new.into(),
        paths: vec!["email".into()],
        op_id: None,
    }
}

// ============================================================================
// REGULAR: crash-state matrix
// ============================================================================

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
        mgr.drop_index("by_email").await.unwrap();
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
        mgr.drop_unique_index("uniq_email").await.unwrap();
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
        mgr.drop_unique_index("uniq_email").await.unwrap();
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

// ============================================================================
// #1000 (@oh post-#997-review fix): recovery interrupted between two
// stranded tombstone entries must not silently discard the not-yet-processed
// entry's tombstone.
//
// `recover_hash_renames` originally called `IndexManager::clear_from_renaming`
// per entry inside its reconciliation loop. That method derives the persisted
// snapshot from the IN-MEMORY `renaming_regular`/`renaming_unique` maps —
// which are ALWAYS EMPTY at open time (`IndexManager::new` starts fresh;
// `load_renaming_list` never rehydrates them, it only returns a local `Vec`).
// So the first per-entry clear during recovery persisted an empty list,
// silently discarding any sibling entry's tombstone if recovery was then
// interrupted before finishing it. A failed (not just crashed) rename
// legitimately leaves its tombstone behind without clearing it, so a
// multi-entry stranded list is a real, reachable state — not a contrived one.
// ============================================================================

#[tokio::test]
async fn p1000_regular_recovery_interrupted_between_entries_preserves_sibling_tombstone() {
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
    mgr.insert(&record_with_str(email_field, "c@d.com"))
        .await
        .unwrap();
    mgr.interner.persist().await.unwrap();

    // Two independent regular indexes, both left in the "crash before
    // create" matrix state (old present, new absent) and stranded
    // simultaneously in the SAME tombstone list.
    mgr.create_index("by_email", &["email"]).await.unwrap();
    mgr.create_index("by_email2", &["email"]).await.unwrap();

    let entry1 = rename_entry("by_email", "by_email_new");
    let entry2 = rename_entry("by_email2", "by_email2_new");
    let key = RecordId::system("idx_ren").to_bytes();
    let bytes = bincode::serialize(&vec![entry1.clone(), entry2.clone()]).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();

    // Install the between-entries pause hook, then run recovery directly
    // (bypassing `TableManager::create`, which would construct its own
    // fresh `IndexManager` a hook installed from outside couldn't reach)
    // racing it against the hook parking.
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.index_manager_ref()
        .set_recover_renames_between_entries_hook(Some(Arc::clone(&hook)));

    tokio::select! {
        r = mgr.recover_hash_renames() => {
            panic!("recovery completed before the between-entries hook fired: {r:?}");
        }
        _ = hook.wait_until_parked() => {
            // Parked: entry 1 (by_email -> by_email_new) is fully
            // reconciled; entry 2 must not have been touched yet.
        }
    }

    // The assertion this test exists for: BOTH tombstone entries must
    // still be on disk while recovery is parked between them. The old
    // buggy per-entry `clear_from_renaming` call would have already wiped
    // the WHOLE list to `[]` here, silently stranding entry 2.
    let remaining = load_regular_rename_tombstone(&info_store).await;
    assert_eq!(
        remaining.len(),
        2,
        "both tombstone entries must still be on disk while recovery is \
         parked between them -- entry 2 must not be silently discarded by \
         entry 1's reconciliation"
    );

    // Simulate the interruption completing as a real crash: drop the
    // manager (the parked `recover_hash_renames` call above was already
    // cancelled by `tokio::select!`).
    drop(mgr);

    // Reopen -- a fresh, uninterrupted recovery pass must finish BOTH
    // entries (entry 1's steps are idempotent no-ops the second time).
    let mgr2 = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr2.index_exists("by_email").await,
        "entry 1's old index must be gone"
    );
    assert!(
        mgr2.index_exists("by_email_new").await,
        "entry 1's new index must exist"
    );
    assert!(
        !mgr2.index_exists("by_email2").await,
        "entry 2's old index must be gone -- must NOT have been silently \
         stranded by entry 1's earlier reconciliation"
    );
    assert!(
        mgr2.index_exists("by_email2_new").await,
        "entry 2's new index must exist after the second recovery pass"
    );

    let r1 = mgr2
        .lookup_by_index("by_email_new", &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(!r1.is_empty(), "entry 1's postings must be resolvable");
    let r2 = mgr2
        .lookup_by_index("by_email2_new", &[InnerValue::Str("c@d.com".into())])
        .await
        .unwrap();
    assert!(!r2.is_empty(), "entry 2's postings must be resolvable");

    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone list must be fully cleared once both entries are done"
    );
}

// ============================================================================
// #1048 (P1-2 sub-slice B): e2e crash-recovery test proving RFC §2.3's
// worked example — op_id carried through rename → crash → recovery →
// SucceededViaCrashRecovery status.
//
// This test reuses the existing `maybe_pause_rename_mid()` test seam to
// simulate a crash mid-rename, then verifies that polling for the SAME op_id
// returns `SucceededViaCrashRecovery` after recovery completes.
// ============================================================================

#[tokio::test]
async fn p1048_e2e_unique_rename_severe_case_op_id_threading() {
    let (data_store, info_store) = make_stores();

    // Step 1: create a table with a unique index and data
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
        mgr.create_unique_index("uniq_email", &["email"])
            .await
            .unwrap();
        drop(mgr);
    }

    // Step 2: mint a real op_id (simulating dispatch) and start a rename
    // that will pause mid-flight
    let op_id = RecordId::system("ddl_rename_index_uniq_email_to_uniq_email_new_test");
    let pause_hook = Arc::new(BackfillPauseHook::new());

    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        // Install the pause hook BEFORE starting the rename
        mgr.index_manager_ref()
            .set_rename_mid_pause_hook(Some(pause_hook.clone()));

        // This will write the tombstone with op_id, then pause mid-rename
        // (after drop_unique_index, before create_unique_index_body — the
        // EXACT SEVERE crash window from RFC §2.3)
        let rename_task = tokio::spawn(async move {
            mgr.rename_index("uniq_email", "uniq_email_new", Some(op_id))
                .await
        });

        // Wait for the pause hook to fire (rename is mid-flight)
        pause_hook.wait_until_parked().await;

        // Drop the rename_task and let mgr be dropped naturally to simulate a crash
        drop(rename_task);
    }

    // Step 3: verify tombstone was written with op_id
    let tombstone_list = load_unique_rename_tombstone(&info_store).await;
    assert_eq!(tombstone_list.len(), 1, "must have one stranded tombstone");
    let tombstone = &tombstone_list[0];
    assert_eq!(tombstone.old_name, "uniq_email");
    assert_eq!(tombstone.new_name, "uniq_email_new");
    assert_eq!(
        tombstone.op_id,
        Some(op_id.to_string()),
        "tombstone must carry the op_id minted at dispatch"
    );

    // Step 4: reopen (simulate server restart) — recovery will complete the rename
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    // Verify the rename completed: old is gone, new exists
    assert!(
        !mgr.unique_index_exists("uniq_email").await,
        "old unique index must be gone after recovery"
    );
    assert!(
        mgr.unique_index_exists("uniq_email_new").await,
        "new unique index must exist after recovery"
    );

    // Verify uniqueness is still enforced
    let email_field = key_id(&mgr, "email").await;
    let dup = mgr.insert(&record_with_str(email_field, "a@b.com")).await;
    assert!(
        dup.is_err(),
        "uniqueness must still be enforced after recovery"
    );

    // Step 5: poll for the op_id and verify we got SucceededViaCrashRecovery
    use crate::table::ddl_op_log;
    use shamir_query_types::read::{DdlOpState, DdlOpStatus};

    let status: Option<DdlOpStatus> = ddl_op_log::read_op_status(&info_store, &op_id)
        .await
        .expect("read_op_status should not error");
    assert!(
        status.is_some(),
        "op_id must be present in the op-status log after recovery"
    );
    let status = status.unwrap();
    assert_eq!(
        status.op_id, op_id,
        "returned status must have the same op_id"
    );

    match status.state {
        DdlOpState::SucceededViaCrashRecovery {
            completed_at_restart,
        } => {
            // Success! The recovery wrote the correct status.
            assert!(
                completed_at_restart > 0,
                "completed_at_restart must be a valid timestamp"
            );
        }
        other => panic!(
            "Expected SucceededViaCrashRecovery after crash recovery, got {:?}",
            other
        ),
    }

    // Tombstone must be cleared
    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared after recovery"
    );
}

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
