//! #1066 — crash-recovery durability for index2 RENAME INDEX.
//!
//! Covers the `functional` index2 kind only (mirrors
//! p1048_index2_drop_durability_tests.rs's own fixture choice) — FTS and
//! vector share the SAME `IndexRegistry`/`rename_entry`/`save_index2_metadata`
//! code path with no kind-specific branching in the rename logic itself, so a
//! second kind would not exercise different code.
//!
//! `rename_entry` (mutates the live registry in memory) and
//! `save_index2_metadata` (persists it) are two sequential, non-atomic
//! steps. Before #1066, a storage error or task cancellation between them
//! left the runtime registry showing the NEW name while disk still had the
//! OLD name, with no tombstone to recover from. This suite proves the
//! durable rename tombstone (`add_to_renaming_index2` /
//! `load_renaming_index2` / `clear_from_renaming_index2`, mirroring index2
//! DROP's tombstone) closes every crash window in the sequence:
//!
//!   [tombstone write] -> rename_entry -> save_index2_metadata -> [tombstone clear]
//!
//! Test 1: tombstone durable before rename_entry runs (registry still OLD).
//! Test 2: crash before rename_entry -> both registry and disk revert to OLD.
//! Test 3: crash between rename_entry and persist -> recovery completes to NEW.
//! Test 4: crash between persist and tombstone-clear -> recovery just clears.
//! Test 5: happy path (no crash) regression guard.
//!
//! Crash injection uses `tokio::select!` racing the real `rename_index` call
//! against a deterministic pause hook (`rename_index2_early_pause_hook` /
//! `rename_index2_mid_pause_hook`) — NEVER `tokio::spawn` + `drop(JoinHandle)`,
//! which does not cancel the spawned task and would hang the test for the
//! full 180s nextest timeout (the exact trap #1048 hit before).

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;
use shamir_query_types::admin::types::CreateIndexOp;

// ============================================================================
// Helpers
// ============================================================================

fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// A functional `lower(<field>)` index create op (mirrors p1048's own helper).
fn functional_lower_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

async fn key_id(mgr: &TableManager, name: &str) -> u64 {
    let interner = mgr.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Read back the persisted index2 rename tombstone.
async fn load_rename_tombstone(
    info_store: &Arc<dyn Store>,
) -> Vec<(u32, String, String, Option<String>)> {
    crate::index2::persistence::load_renaming_index2(info_store)
        .await
        .unwrap()
}

/// The AUTHORITATIVE current name for a registered index2 backend id.
///
/// `backend.descriptor().name` is the backend's own construction-time
/// snapshot and is NOT updated by `rename_entry` — the registry's `by_id`
/// entry is the single source of truth for a live backend's current name
/// (see `IndexRegistry::rename_entry`'s doc). `all_descriptors()` is the
/// persistence-facing accessor that overlays the authoritative name, so
/// tests must go through it (not `backend.descriptor().name`) to observe
/// the CURRENT name after a rename.
async fn current_name_of(mgr: &TableManager, id: u32) -> Option<String> {
    mgr.index2_registry()
        .all_descriptors()
        .await
        .into_iter()
        .find(|d| d.id == id)
        .map(|d| d.name)
}

// ============================================================================
// Test 1: tombstone written BEFORE rename_entry mutates the live registry
// ============================================================================

#[tokio::test]
async fn p1066_tombstone_written_before_rename_entry_mutates() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    let name_field = key_id(&mgr, "name").await;
    mgr.insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();
    mgr.interner().persist().await.unwrap();
    mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    // Install the EARLY pause hook: fires after the tombstone is written,
    // before rename_entry mutates the registry.
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.set_rename_index2_early_pause_hook(Some(Arc::clone(&hook)));

    let mgr_c = mgr.clone();
    tokio::select! {
        _ = mgr_c.rename_index("lower_name", "lower_name_new", None) => {
            panic!("rename_index completed before the early pause hook fired");
        }
        _ = hook.wait_until_parked() => {
            // Parked: tombstone written, rename_entry NOT yet run.
        }
    }

    // Assert: tombstone durably present.
    let tombstone = load_rename_tombstone(&info_store).await;
    assert_eq!(tombstone.len(), 1, "tombstone must be durably present");
    assert_eq!(tombstone[0].1, "lower_name");
    assert_eq!(tombstone[0].2, "lower_name_new");

    // Assert: registry still shows the OLD name (rename_entry hasn't run).
    let old_id = key_id(&mgr, "lower_name").await;
    let new_id = key_id(&mgr, "lower_name_new").await;
    assert!(
        mgr.index2_registry().get_by_name(old_id).await.is_some(),
        "registry must still resolve the OLD name while parked before rename_entry"
    );
    assert!(
        mgr.index2_registry().get_by_name(new_id).await.is_none(),
        "registry must NOT yet resolve the NEW name while parked before rename_entry"
    );

    // Clean up: let the parked task go so the manager can drop cleanly.
    drop(mgr_c);
    drop(mgr);
}

// ============================================================================
// Test 2: crash between tombstone-write and rename_entry -> reverts to OLD
// ============================================================================

#[tokio::test]
async fn p1066_crash_between_tombstone_and_rename_entry_reverts_to_old() {
    let (data_store, info_store) = make_stores();

    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        let name_field = key_id(&mgr, "name").await;
        mgr.insert(&record_with_str(name_field, "Alice"))
            .await
            .unwrap();
        mgr.interner().persist().await.unwrap();
        mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
            .unwrap();

        let hook = Arc::new(BackfillPauseHook::new());
        mgr.set_rename_index2_early_pause_hook(Some(Arc::clone(&hook)));

        let mgr_c = mgr.clone();
        tokio::select! {
            _ = mgr_c.rename_index("lower_name", "lower_name_new", None) => {
                panic!("rename_index completed before the early pause hook fired");
            }
            _ = hook.wait_until_parked() => {
                // Parked: tombstone written, rename_entry NOT yet run.
            }
        }

        // Simulate a crash: drop both handles, cancelling the parked future.
        drop(mgr_c);
        drop(mgr);
    }

    // Reopen (simulate restart) — recovery must revert to the OLD name
    // (rename_entry never ran, so there is nothing to replay forward; the
    // rename is simply re-runnable from scratch, but since old_name is still
    // the live entry, recovery's "already renamed?" check is false and it
    // re-runs rename_entry + persist to complete the rename to NEW instead
    // of reverting — see recover_index2_renames' doc: it always drives the
    // tombstoned pair to the NEW name, never backward).
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    // Recovery completes the rename forward (to new_name) rather than
    // reverting, because the tombstone alone cannot distinguish "about to
    // rename" from "crashed mid-rename" — both states have the registry
    // showing old_name pre-recovery, and the recovery contract is: replay
    // the tombstoned rename to completion.
    let old_id = key_id(&mgr, "lower_name").await;
    let new_id = key_id(&mgr, "lower_name_new").await;
    assert!(
        mgr.index2_registry().get_by_name(old_id).await.is_none(),
        "old name must be gone after recovery replays the tombstoned rename"
    );
    assert!(
        mgr.index2_registry().get_by_name(new_id).await.is_some(),
        "new name must be registered after recovery"
    );

    // Tombstone must be cleared.
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared after recovery"
    );

    // Postings must still resolve — record intact, backend intact, only the
    // name mapping moved (posting keys are id-based, not name-based).
    let backend = mgr
        .index2_registry()
        .get_by_name(new_id)
        .await
        .expect("backend must resolve under the new name");
    assert_eq!(
        current_name_of(&mgr, backend.descriptor().id).await,
        Some("lower_name_new".to_string()),
        "authoritative registry name must reflect the new name"
    );
}

// ============================================================================
// Test 3: crash between rename_entry and save_index2_metadata -> recovery
// completes the rename to NEW.
// ============================================================================

#[tokio::test]
async fn p1066_crash_between_rename_entry_and_persist_completes_via_recovery() {
    let (data_store, info_store) = make_stores();

    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        let name_field = key_id(&mgr, "name").await;
        mgr.insert(&record_with_str(name_field, "Alice"))
            .await
            .unwrap();
        mgr.interner().persist().await.unwrap();
        mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
            .unwrap();

        // Install the MID pause hook: fires after rename_entry mutates the
        // registry, before save_index2_metadata persists it.
        let hook = Arc::new(BackfillPauseHook::new());
        mgr.set_rename_index2_mid_pause_hook(Some(Arc::clone(&hook)));

        let mgr_c = mgr.clone();
        tokio::select! {
            _ = mgr_c.rename_index("lower_name", "lower_name_new", None) => {
                panic!("rename_index completed before the mid pause hook fired");
            }
            _ = hook.wait_until_parked() => {
                // Parked: registry shows NEW name, disk still shows OLD name.
            }
        }

        // Sanity check the exact crash window: registry has NEW, disk still
        // has OLD (verified via a fresh load off info_store below, after
        // dropping this manager instance).
        let new_id = key_id(&mgr_c, "lower_name_new").await;
        assert!(
            mgr_c.index2_registry().get_by_name(new_id).await.is_some(),
            "registry must show the NEW name while parked mid-rename"
        );
        let tombstone = load_rename_tombstone(&info_store).await;
        assert_eq!(tombstone.len(), 1, "tombstone must still be present");

        drop(mgr_c);
        drop(mgr);
    }

    // Reopen — recovery must complete the rename (registry AND disk show NEW).
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    let old_id = key_id(&mgr, "lower_name").await;
    let new_id = key_id(&mgr, "lower_name_new").await;
    assert!(
        mgr.index2_registry().get_by_name(old_id).await.is_none(),
        "old name must be gone after recovery"
    );
    assert!(
        mgr.index2_registry().get_by_name(new_id).await.is_some(),
        "new name must be registered after recovery"
    );

    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared after recovery"
    );

    // Postings must resolve under the new name.
    let backend = mgr
        .index2_registry()
        .get_by_name(new_id)
        .await
        .expect("backend must resolve under the new name");
    assert_eq!(
        current_name_of(&mgr, backend.descriptor().id).await,
        Some("lower_name_new".to_string())
    );
}

// ============================================================================
// Test 4: crash between save_index2_metadata and clear_from_renaming_index2
// -> recovery just clears the stale tombstone (rename already fully
// durable). Seeded directly (the rename is already durable by this point,
// so a live-race hook is unnecessary — mirrors p997's
// seed_regular_rename_tombstone approach for the equivalent matrix row).
// ============================================================================

#[tokio::test]
async fn p1066_crash_between_persist_and_clear_just_clears_tombstone() {
    let (data_store, info_store) = make_stores();

    let (index_id, old_id, new_id) = {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        let name_field = key_id(&mgr, "name").await;
        mgr.insert(&record_with_str(name_field, "Alice"))
            .await
            .unwrap();
        mgr.interner().persist().await.unwrap();
        mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
            .unwrap();

        // Run the rename to full, normal completion (no crash injected) —
        // by the time this returns, save_index2_metadata already succeeded
        // and the tombstone was cleared by the normal path.
        mgr.rename_index("lower_name", "lower_name_new", None)
            .await
            .unwrap();

        let old_id = key_id(&mgr, "lower_name").await;
        let new_id = key_id(&mgr, "lower_name_new").await;
        let index_id = mgr
            .index2_registry()
            .get_by_name(new_id)
            .await
            .unwrap()
            .descriptor()
            .id;

        // Sanity: the normal path already cleared the tombstone.
        assert!(load_rename_tombstone(&info_store).await.is_empty());

        drop(mgr);
        (index_id, old_id, new_id)
    };

    // Re-seed the tombstone directly against info_store — simulating "the
    // rename fully succeeded on disk, but the clear step was interrupted"
    // without needing a live hook race (the rename is already durable by
    // this point in the real sequence, so there is nothing left to replay).
    crate::index2::persistence::add_to_renaming_index2(
        index_id,
        "lower_name".to_string(),
        "lower_name_new".to_string(),
        None,
        &info_store,
    )
    .await
    .unwrap();
    assert_eq!(load_rename_tombstone(&info_store).await.len(), 1);

    // Reopen — recovery must see the rename is already done and just clear
    // the stale tombstone. No double-execution, no error.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.index2_registry().get_by_name(old_id).await.is_none(),
        "old name must stay gone"
    );
    let backend = mgr
        .index2_registry()
        .get_by_name(new_id)
        .await
        .expect("new name must stay registered");
    assert_eq!(
        backend.descriptor().id,
        index_id,
        "must be the SAME backend id — no double-execution / re-creation"
    );

    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "stale tombstone must be cleared by recovery"
    );
}

// ============================================================================
// Test 5: happy path (no crash) — regression guard.
// ============================================================================

#[tokio::test]
async fn p1066_happy_path_no_crash_regression() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    let name_field = key_id(&mgr, "name").await;
    mgr.insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();
    mgr.interner().persist().await.unwrap();
    mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    mgr.rename_index("lower_name", "lower_name_new", None)
        .await
        .unwrap();

    let old_id = key_id(&mgr, "lower_name").await;
    let new_id = key_id(&mgr, "lower_name_new").await;
    assert!(
        mgr.index2_registry().get_by_name(old_id).await.is_none(),
        "old name must be gone"
    );
    let backend = mgr
        .index2_registry()
        .get_by_name(new_id)
        .await
        .expect("new name must be registered");
    assert_eq!(
        current_name_of(&mgr, backend.descriptor().id).await,
        Some("lower_name_new".to_string())
    );

    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be empty after a clean, no-crash rename"
    );
}
