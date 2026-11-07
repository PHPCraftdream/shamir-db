//! P0-5b (#962): RENAME INDEX durable tombstone + crash recovery tests for the
//! SORTED index family.
//!
//! Mirrors #972's `p03b_sorted_drop_durability_tests.rs` one-to-one, but for
//! RENAME (the metadata is re-pointed `old_id → new_id` instead of retired, and
//! the physical postings are REKEYED to the new prefix instead of swept away).
//! Tests the three crash states from the bug brief:
//! - **crash after `rename_definition`, before rekey** (the headline bug):
//!   definition already under `new_id`, postings STILL stranded under `old_id`.
//!   Recovery must finish the rekey and clear the tombstone.
//! - **crash before `rename_definition`**: definition still under `old_id`,
//!   postings under `old_id`. Recovery must rename the definition AND rekey.
//! - **idempotent resume**: a second restart after recovery is a clean no-op.
//!
//! Plus a live-crash test (park at the rename→rekey pause hook) and a
//! regression test that a normal rename leaves no tombstone behind.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::base_index::backfill_pause_hook::BackfillPauseHook;
use crate::base_index::sorted_index_definition::{SortedIndexDefinition, SORTED_TAG};
use crate::base_index::sorted_index_manager::SortedIndexManager;

// ============================================================================
// Helpers (mirror p03b's helpers, adapted for rename)
// ============================================================================

/// Fresh in-memory info_store (sorted defs AND postings both live here).
fn fresh_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new()) as Arc<dyn Store>
}

/// Build `{ field_key: Int(score) }`.
fn int_record(field_key: u64, score: i64) -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(field_key), InnerValue::Int(score));
    InnerValue::Map(m)
}

/// The physical posting-key prefix for one sorted index:
/// `[SORTED_TAG] ++ name_interned.to_be_bytes()`.
fn sorted_prefix(name_interned: u64) -> Bytes {
    let mut buf = Vec::with_capacity(9);
    buf.push(SORTED_TAG);
    buf.extend_from_slice(&name_interned.to_be_bytes());
    Bytes::from(buf)
}

/// Count posting entries under one sorted index's prefix.
async fn count_postings(info_store: &Arc<dyn Store>, name_interned: u64) -> usize {
    let prefix = sorted_prefix(name_interned);
    let stream = info_store.scan_prefix_stream(prefix, 1000);
    futures::pin_mut!(stream);
    let mut count = 0;
    while let Some(batch) = stream.next().await {
        for (_, _) in batch.unwrap() {
            count += 1;
        }
    }
    count
}

/// Build a Ready sorted index + its postings via the REAL register +
/// on_record_created path, under `name_interned`. Returns nothing — the
/// on-disk defs + postings persist in `info_store` after the manager drops.
async fn seed_index_and_postings(
    info_store: &Arc<dyn Store>,
    name_interned: u64,
    field_key: u64,
    scores: &[i64],
) {
    let mgr = SortedIndexManager::new(Arc::clone(info_store))
        .await
        .unwrap();
    mgr.register(SortedIndexDefinition::new(name_interned, vec![field_key]))
        .await
        .unwrap();
    for &score in scores {
        let id = RecordId::new();
        let rec = int_record(field_key, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }
    assert!(mgr.find_by_name_interned(name_interned).is_some());
}

/// Write a "Renaming" tombstone directly into info_store, simulating the
/// persisted state after `add_to_renaming_sorted` but before the rekey
/// completes (or before `rename_definition` runs).
async fn seed_rename_tombstone(info_store: &Arc<dyn Store>, pairs: &[(u64, u64)]) {
    let key = RecordId::system("sidx_ren").to_bytes();
    let bytes = bincode::serialize(pairs).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
}

/// Read back the persisted "Renaming" tombstone (empty vec if absent).
async fn load_rename_tombstone(info_store: &Arc<dyn Store>) -> Vec<(u64, u64)> {
    let key = RecordId::system("sidx_ren").to_bytes();
    match info_store.get(key.into()).await {
        Ok(bytes) if bytes.is_empty() => Vec::new(),
        Ok(bytes) => bincode::deserialize(&bytes).unwrap(),
        Err(_) => Vec::new(),
    }
}

// ============================================================================
// Headline bug: crash AFTER rename_definition, BEFORE rekey
// (definition already under new_id, postings STILL under old_id)
// ============================================================================

#[tokio::test]
async fn p05b_crash_after_rename_def_before_rekey() {
    // Crash state: tombstone written, rename_definition ran (def now under
    // new_id, persisted), but the process crashed before `rekey_postings`
    // moved the postings. On-disk: def under new_id, postings under old_id,
    // tombstone present. THIS IS THE EXACT BUG P0-5b DESCRIBES.
    let info_store = fresh_store();
    let old_id = 5101u64;
    let new_id = 5102u64;

    // Seed the index + postings under old_id.
    seed_index_and_postings(&info_store, old_id, 1, &[10, 20, 30]).await;
    assert_eq!(
        count_postings(&info_store, old_id).await,
        3,
        "precondition: 3 postings seeded under old_id"
    );

    // rename_definition swaps the def old_id → new_id and persists.
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.rename_definition(old_id, new_id).await.unwrap();
        assert!(mgr.find_by_name_interned(new_id).is_some());
        assert!(mgr.find_by_name_interned(old_id).is_none());
    }
    // Postings are STILL under old_id (rekey never ran).
    assert_eq!(
        count_postings(&info_store, old_id).await,
        3,
        "postings still under old_id after rename_definition"
    );
    assert_eq!(
        count_postings(&info_store, new_id).await,
        0,
        "no postings under new_id yet"
    );

    // Tombstone written (the rename was interrupted right here).
    seed_rename_tombstone(&info_store, &[(old_id, new_id)]).await;

    // Construct a fresh manager — recovery MUST finish the rekey.
    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.find_by_name_interned(new_id).is_some(),
        "def must remain under new_id after recovery"
    );
    assert!(
        mgr.find_by_name_interned(old_id).is_none(),
        "old_id must NOT have a def after recovery"
    );
    assert_eq!(
        count_postings(&info_store, new_id).await,
        3,
        "P0-5b FAIL: recovery must move postings to new_id"
    );
    assert_eq!(
        count_postings(&info_store, old_id).await,
        0,
        "P0-5b FAIL: no postings may be orphaned under old_id"
    );
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "P0-5b FAIL: tombstone must be cleared after recovery"
    );
}

// ============================================================================
// Crash BEFORE rename_definition (def still under old_id, postings under old_id)
// ============================================================================

#[tokio::test]
async fn p05b_crash_before_rename_def_postings_under_old() {
    // Crash state: tombstone written, crashed BEFORE rename_definition. Def
    // still under old_id, postings under old_id. Recovery must rename the def
    // AND rekey the postings.
    let info_store = fresh_store();
    let old_id = 6101u64;
    let new_id = 6102u64;

    seed_index_and_postings(&info_store, old_id, 1, &[1, 2]).await;
    assert_eq!(count_postings(&info_store, old_id).await, 2);

    // Tombstone only — do NOT rename the definition.
    seed_rename_tombstone(&info_store, &[(old_id, new_id)]).await;

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.find_by_name_interned(new_id).is_some(),
        "recovery must rename the def to new_id"
    );
    assert!(
        mgr.find_by_name_interned(old_id).is_none(),
        "old_id def must be gone after recovery renamed it"
    );
    assert_eq!(
        count_postings(&info_store, new_id).await,
        2,
        "recovery must rekey postings to new_id"
    );
    assert_eq!(
        count_postings(&info_store, old_id).await,
        0,
        "no orphan postings under old_id"
    );
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "tombstone cleared after recovery"
    );
}

// ============================================================================
// Idempotent resume — two restart attempts
// ============================================================================

#[tokio::test]
async fn p05b_idempotent_resume_double_restart() {
    let info_store = fresh_store();
    let old_id = 7101u64;
    let new_id = 7102u64;

    seed_index_and_postings(&info_store, old_id, 1, &[5, 6]).await;
    // Crash state: def renamed, rekey not run.
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.rename_definition(old_id, new_id).await.unwrap();
    }
    seed_rename_tombstone(&info_store, &[(old_id, new_id)]).await;

    // First restart — recovery runs.
    let mgr1 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr1.find_by_name_interned(new_id).is_some());
    assert_eq!(count_postings(&info_store, new_id).await, 2);
    assert!(load_rename_tombstone(&info_store).await.is_empty());

    // Second restart — must be a clean no-op, not an error or double-move.
    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(
        mgr2.find_by_name_interned(new_id).is_some(),
        "idempotent: def still under new_id after second restart"
    );
    assert!(
        mgr2.find_by_name_interned(old_id).is_none(),
        "idempotent: old_id def still absent"
    );
    assert_eq!(
        count_postings(&info_store, new_id).await,
        2,
        "idempotent: postings unchanged after second restart"
    );
    assert_eq!(
        count_postings(&info_store, old_id).await,
        0,
        "idempotent: no postings leaked back to old_id"
    );
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "idempotent: tombstone empty after second restart"
    );
}

// ============================================================================
// Live rename crash at the rename→rekey pause hook
// ============================================================================

#[tokio::test]
async fn p05b_live_rename_crash_at_rekey_hook() {
    let info_store = fresh_store();
    let old_id = 8101u64;
    let new_id = 8102u64;

    // Create and populate the index via the real path.
    seed_index_and_postings(&info_store, old_id, 1, &[1, 2]).await;
    assert_eq!(count_postings(&info_store, old_id).await, 2);

    // Construct a manager and install the rename→rekey pause hook.
    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.set_rename_rekey_pause_hook(Some(Arc::clone(&hook)));

    // Start rename_index_sorted and let it park at the rekey hook
    // (rename_definition already swapped + persisted the def to new_id).
    let mgr_clone = mgr.clone();
    tokio::select! {
        _ = mgr_clone.rename_index_sorted(old_id, new_id, None, None, None) => {
            panic!("rename_index_sorted completed before the rekey hook fired");
        }
        _ = hook.wait_until_parked() => {
            // Parked: def under new_id, postings NOT yet rekeyed.
        }
    }

    // Verify the parked window matches the bug's crash state.
    assert_eq!(
        count_postings(&info_store, old_id).await,
        2,
        "parked window: postings still under old_id"
    );

    // Simulate crash: drop the manager (its in-memory state dies). The select
    // already cancelled the rename_index_sorted future.
    drop(mgr_clone);
    drop(mgr);

    // Construct a fresh manager — recovery MUST finish the rekey.
    let new_mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        new_mgr.find_by_name_interned(new_id).is_some(),
        "LIVE: def under new_id after recovery"
    );
    assert!(
        new_mgr.find_by_name_interned(old_id).is_none(),
        "LIVE: old_id def absent after recovery"
    );
    assert_eq!(
        count_postings(&info_store, new_id).await,
        2,
        "LIVE: recovery moved postings to new_id"
    );
    assert_eq!(
        count_postings(&info_store, old_id).await,
        0,
        "LIVE: no orphan postings under old_id"
    );
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "LIVE: tombstone cleared after recovery"
    );
}

// ============================================================================
// Normal rename leaves NO tombstone behind (regression)
// ============================================================================

#[tokio::test]
async fn p05b_normal_rename_leaves_no_tombstone() {
    let info_store = fresh_store();
    let old_id = 9101u64;
    let new_id = 9102u64;

    seed_index_and_postings(&info_store, old_id, 1, &[7, 8, 9]).await;
    assert_eq!(count_postings(&info_store, old_id).await, 3);

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr.find_by_name_interned(old_id).is_some());

    // Full rename via the public orchestration entry point.
    mgr.rename_index_sorted(old_id, new_id, None, None, None)
        .await
        .unwrap();

    assert!(
        mgr.find_by_name_interned(new_id).is_some(),
        "def under new_id after rename"
    );
    assert!(
        mgr.find_by_name_interned(old_id).is_none(),
        "old_id def gone after rename"
    );
    assert_eq!(
        count_postings(&info_store, new_id).await,
        3,
        "postings rekeyed to new_id"
    );
    assert_eq!(
        count_postings(&info_store, old_id).await,
        0,
        "no postings left under old_id"
    );

    // The tombstone must NOT be left behind on a successful rename.
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "regression: tombstone must not linger after a successful rename"
    );

    // A fresh manager must load the renamed state cleanly (no recovery needed).
    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(
        mgr2.find_by_name_interned(new_id).is_some(),
        "fresh manager loads renamed def"
    );
    assert!(mgr2.find_by_name_interned(old_id).is_none());
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "fresh manager sees no tombstone"
    );
}

// ============================================================================
// Recovery does not disturb an unrelated surviving index
// ============================================================================

#[tokio::test]
async fn p05b_recovery_does_not_affect_surviving_indexes() {
    let info_store = fresh_store();
    let old_id = 11101u64;
    let new_id = 11102u64;
    let surviving_id = 11103u64;

    // Seed the renamed index (under old_id) AND an unrelated survivor.
    seed_index_and_postings(&info_store, old_id, 1, &[100]).await;
    seed_index_and_postings(&info_store, surviving_id, 2, &[200]).await;
    assert_eq!(count_postings(&info_store, old_id).await, 1);
    assert_eq!(count_postings(&info_store, surviving_id).await, 1);

    // Crash state: def renamed, rekey not run.
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.rename_definition(old_id, new_id).await.unwrap();
    }
    seed_rename_tombstone(&info_store, &[(old_id, new_id)]).await;

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    // Renamed index recovered.
    assert!(mgr.find_by_name_interned(new_id).is_some());
    assert_eq!(count_postings(&info_store, new_id).await, 1);
    assert_eq!(count_postings(&info_store, old_id).await, 0);

    // Survivor untouched.
    assert!(
        mgr.find_by_name_interned(surviving_id).is_some(),
        "surviving index must NOT be affected by rename recovery"
    );
    assert_eq!(
        count_postings(&info_store, surviving_id).await,
        1,
        "surviving index postings intact"
    );
    assert!(
        load_rename_tombstone(&info_store).await.is_empty(),
        "tombstone cleared after recovery"
    );
}
