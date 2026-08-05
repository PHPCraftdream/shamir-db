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
        mgr.rename_index("uniq_email", "uniq_email_new")
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
        _ = mgr_c.rename_index("by_email", "by_email_new") => {
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
        _ = mgr_c.rename_index("uniq_email", "uniq_email_new") => {
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
    mgr.rename_index("by_email", "by_email_new").await.unwrap();

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
    mgr.rename_index("uniq_email", "uniq_email_new")
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
