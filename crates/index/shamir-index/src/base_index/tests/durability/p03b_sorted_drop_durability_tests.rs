//! P0-3b (#972): DROP INDEX durable tombstone + crash recovery tests for the
//! SORTED index family.
//!
//! Mirrors #959's `p03_drop_durability_tests.rs` (base_index regular/unique
//! family) one-to-one for sorted. Tests the three sub-bugs:
//! - **3c** (crash-resurrection): a crash between sweep and defs-persist must
//!   NOT resurrect a fully-broken "Ready but no postings" index.
//! - **3c** (idempotent resume): calling the recovery path twice must be a
//!   clean no-op on the second call.
//! - **3b** (name-reuse ghost postings): a `register` reusing a name whose
//!   DROP is still in flight must be rejected until the tombstone clears.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::base_index::backfill_pause_hook::BackfillPauseHook;
use crate::base_index::sorted_index_definition::{SortedIndexDefinition, SORTED_TAG};
use crate::base_index::sorted_index_manager::SortedIndexManager;

// ============================================================================
// Helpers
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

/// Remove every posting under one sorted index's prefix directly from the
/// store — used to simulate the DROP's sweep step having ALREADY run before
/// the simulated crash.
async fn sweep_postings_direct(info_store: &Arc<dyn Store>, name_interned: u64) {
    let prefix = sorted_prefix(name_interned);
    let stream = info_store.scan_prefix_stream(prefix, 1000);
    futures::pin_mut!(stream);
    let mut keys: Vec<RecordKey> = Vec::new();
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.unwrap() {
            keys.push(k);
        }
    }
    info_store.remove_many(keys).await.unwrap();
}

/// Write a tombstone directly into info_store, simulating the persisted state
/// after `add_to_dropping_sorted` but before the sweep/persist completes.
async fn seed_tombstone(info_store: &Arc<dyn Store>, names: &[u64]) {
    let key = RecordId::system("sidx_drop").to_bytes();
    let bytes = bincode::serialize(names).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
}

/// Read back the persisted tombstone (empty vec if absent).
async fn load_tombstone(info_store: &Arc<dyn Store>) -> Vec<u64> {
    let key = RecordId::system("sidx_drop").to_bytes();
    match info_store.get(key.into()).await {
        Ok(bytes) if bytes.is_empty() => Vec::new(),
        Ok(bytes) => bincode::deserialize(&bytes).unwrap(),
        Err(_) => Vec::new(),
    }
}

/// Build a Ready sorted index + its postings via the REAL register +
/// on_record_created path, then drop the manager handle (the on-disk defs +
/// postings persist in `info_store`).
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

// ============================================================================
// 3c — crash-resurrection recovery (direct state setup)
// ============================================================================

#[tokio::test]
async fn p03b_3c_crash_after_sweep_does_not_resurrect() {
    // Crash state: index was Ready with postings, DROP started (tombstone
    // written, postings swept), but the process crashed before the reduced
    // defs were persisted. On-disk: old defs (def still present), tombstone
    // present, postings gone.
    let info_store = fresh_store();
    let name_interned = 5001u64;

    seed_index_and_postings(&info_store, name_interned, 1, &[10, 20, 30]).await;
    assert_eq!(
        count_postings(&info_store, name_interned).await,
        3,
        "precondition: 3 postings seeded"
    );

    // Simulate the DROP's sweep step having already run.
    sweep_postings_direct(&info_store, name_interned).await;
    assert_eq!(count_postings(&info_store, name_interned).await, 0);

    // Tombstone written, reduced defs NOT yet persisted.
    seed_tombstone(&info_store, &[name_interned]).await;

    // Construct a fresh manager — recovery MUST run.
    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    // The index must NOT be visible (no resurrection).
    assert!(
        mgr.find_by_name_interned(name_interned).is_none(),
        "3c FAIL: dropped sorted index was resurrected as Ready after crash"
    );
    assert!(
        !mgr.has_indexes(),
        "3c FAIL: has_indexes should be false after recovery"
    );
    assert_eq!(
        count_postings(&info_store, name_interned).await,
        0,
        "3c FAIL: postings must stay swept"
    );

    // Tombstone must be cleared.
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c FAIL: tombstone should be cleared after recovery"
    );
}

// ============================================================================
// 3c — crash between tombstone-write and sweep (postings still present)
// ============================================================================

#[tokio::test]
async fn p03b_3c_crash_before_sweep_postings_still_present() {
    // Crash state: DROP started (tombstone written), crashed BEFORE the
    // sweep. Postings still intact. Recovery must sweep + remove def.
    let info_store = fresh_store();
    let name_interned = 8001u64;

    seed_index_and_postings(&info_store, name_interned, 1, &[1, 2]).await;
    assert_eq!(
        count_postings(&info_store, name_interned).await,
        2,
        "precondition: 2 postings seeded"
    );

    // Tombstone only — do NOT sweep (simulating crash between tombstone and
    // sweep).
    seed_tombstone(&info_store, &[name_interned]).await;

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.find_by_name_interned(name_interned).is_none(),
        "3c FAIL: index should be gone after recovery retired it"
    );

    // Postings must be swept by recovery.
    assert_eq!(
        count_postings(&info_store, name_interned).await,
        0,
        "3c FAIL: recovery must sweep postings"
    );
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c FAIL: tombstone cleared after recovery"
    );
}

// ============================================================================
// 3c — crash after persist but before tombstone clear
// ============================================================================

#[tokio::test]
async fn p03b_3c_crash_after_persist_before_tombstone_clear() {
    // Crash state: DROP fully completed (def removed from defs, postings
    // swept, reduced defs persisted), but crashed before tombstone was
    // cleared. Recovery should just clear the stale tombstone (no-op sweep).
    let info_store = fresh_store();
    let name_interned = 9001u64;

    // Persist an EMPTY defs blob (the drop already finalized the removal).
    let empty_defs: Vec<SortedIndexDefinition> = Vec::new();
    let key = RecordId::system("sorted_indexes").to_bytes();
    let bytes = bincode::serialize(&empty_defs).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
    // Tombstone still present.
    seed_tombstone(&info_store, &[name_interned]).await;

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.find_by_name_interned(name_interned).is_none(),
        "3c: index gone (def already removed from defs before crash)"
    );

    // Tombstone must be cleared by recovery.
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c: stale tombstone cleared after recovery"
    );
}

// ============================================================================
// 3c — idempotent resume (two restart attempts)
// ============================================================================

#[tokio::test]
async fn p03b_3c_idempotent_resume_double_restart() {
    let info_store = fresh_store();
    let name_interned = 7001u64;

    seed_index_and_postings(&info_store, name_interned, 1, &[5, 6]).await;

    // Sweep + tombstone (crash state: sweep ran, persist did not).
    sweep_postings_direct(&info_store, name_interned).await;
    seed_tombstone(&info_store, &[name_interned]).await;

    // First restart — recovery runs.
    let mgr1 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr1.find_by_name_interned(name_interned).is_none());

    // Second restart — must be a clean no-op, not an error or double-sweep.
    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(
        mgr2.find_by_name_interned(name_interned).is_none(),
        "3c idempotent: index still gone after second restart"
    );
    assert!(
        !mgr2.has_indexes(),
        "3c idempotent: has_indexes false after second restart"
    );
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c idempotent: tombstone empty after second restart"
    );
}

// ============================================================================
// 3c — live DROP with post-sweep hook: simulates real crash mid-operation
// ============================================================================

#[tokio::test]
async fn p03b_3c_live_drop_crash_at_post_sweep_hook() {
    let info_store = fresh_store();
    let name_interned = 10001u64;

    // Create and populate the index via the real path.
    seed_index_and_postings(&info_store, name_interned, 1, &[1, 2]).await;
    assert_eq!(
        count_postings(&info_store, name_interned).await,
        2,
        "precondition: 2 postings after create"
    );

    // Construct a manager and install the post-sweep hook.
    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.set_drop_index_post_sweep_hook(Some(Arc::clone(&hook)));

    // Start drop_index and let it park at the post-sweep hook (sweep done,
    // reduced defs NOT yet persisted).
    let mgr_clone = mgr.clone();
    tokio::select! {
        _ = mgr_clone.drop_index(name_interned, None, None) => {
            panic!("drop_index completed before post-sweep hook fired");
        }
        _ = hook.wait_until_parked() => {
            // Parked: sweep done, defs not yet persisted.
        }
    }

    // Simulate crash: drop the manager (its in-memory state dies). The select
    // already cancelled the drop_index future.
    drop(mgr_clone);
    drop(mgr);

    // Construct a fresh manager — recovery MUST finish the drop.
    let new_mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        new_mgr.find_by_name_interned(name_interned).is_none(),
        "3c LIVE: dropped sorted index must not be resurrected after simulated crash"
    );
    assert!(
        !new_mgr.has_indexes(),
        "3c LIVE: has_indexes must be false after recovery"
    );
    assert_eq!(
        count_postings(&info_store, name_interned).await,
        0,
        "3c LIVE: postings swept by recovery"
    );
}

// ============================================================================
// 3b — name-reuse rejection during in-flight DROP
// ============================================================================

#[tokio::test]
async fn p03b_3b_name_reuse_rejected_during_drop() {
    let info_store = fresh_store();
    let name_interned = 11001u64;

    // Create and populate the index.
    seed_index_and_postings(&info_store, name_interned, 1, &[1, 2]).await;

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    // Install the pre-sweep pause hook (fires after tombstone-write + retire,
    // before the sweep).
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.set_drop_index_pause_hook(Some(Arc::clone(&hook)));

    // Start drop in a background task.
    let mgr_clone = mgr.clone();
    let drop_task = tokio::spawn(async move {
        mgr_clone
            .drop_index(name_interned, None, None)
            .await
            .unwrap();
    });

    // Wait for the drop to park (tombstone written, def retired, pre-sweep).
    hook.wait_until_parked().await;

    // Attempt register with the same name — MUST be rejected.
    let create_result = mgr
        .register(SortedIndexDefinition::new(name_interned, vec![1]))
        .await;
    assert!(
        create_result.is_err(),
        "3b FAIL: register during in-flight DROP must be rejected"
    );

    // Release the hook and let DROP complete.
    hook.release();
    drop_task.await.unwrap();

    // After DROP completes (tombstone cleared), register with the same name
    // should succeed.
    let create_result = mgr
        .register(SortedIndexDefinition::new(name_interned, vec![1]))
        .await;
    assert!(
        create_result.is_ok(),
        "3b: register after DROP completes must succeed, got: {:?}",
        create_result.err()
    );
    assert!(mgr.find_by_name_interned(name_interned).is_some());
}

// ============================================================================
// Normal DROP still works (smoke test — no regression from tombstone changes)
// ============================================================================

#[tokio::test]
async fn p03b_normal_drop_still_works() {
    let info_store = fresh_store();
    let name_interned = 13001u64;

    seed_index_and_postings(&info_store, name_interned, 1, &[7, 8, 9]).await;
    assert_eq!(count_postings(&info_store, name_interned).await, 3);

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr.find_by_name_interned(name_interned).is_some());

    let removed = mgr.drop_index(name_interned, None, None).await.unwrap();
    assert!(removed);
    assert!(mgr.find_by_name_interned(name_interned).is_none());
    assert!(!mgr.has_indexes());
    assert_eq!(
        count_postings(&info_store, name_interned).await,
        0,
        "postings swept after normal drop"
    );

    // Tombstone must be cleared after a normal drop.
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "tombstone must not contain the dropped name after normal drop"
    );

    // A fresh manager must load clean state (no resurrection).
    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr2.find_by_name_interned(name_interned).is_none());
}

// ============================================================================
// Surviving indexes are NOT affected by recovery of a different name
// ============================================================================

#[tokio::test]
async fn p03b_3c_recovery_does_not_affect_surviving_indexes() {
    let info_store = fresh_store();
    let dropped_name = 15001u64;
    let surviving_name = 15002u64;

    // Seed two sorted indexes (different field paths so postings don't
    // collide).
    seed_index_and_postings(&info_store, dropped_name, 1, &[100]).await;
    seed_index_and_postings(&info_store, surviving_name, 2, &[200]).await;
    assert_eq!(count_postings(&info_store, dropped_name).await, 1);
    assert_eq!(count_postings(&info_store, surviving_name).await, 1);

    // Tombstone only the dropped one (simulating crash before sweep).
    seed_tombstone(&info_store, &[dropped_name]).await;

    // Construct fresh manager — recovery must drop only `dropped_name`.
    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.find_by_name_interned(dropped_name).is_none(),
        "dropped index should be gone"
    );
    assert!(
        mgr.find_by_name_interned(surviving_name).is_some(),
        "surviving index must NOT be affected by recovery"
    );
    assert!(
        mgr.has_indexes(),
        "has_indexes must be true (surviving index present)"
    );

    // Surviving index's postings must be intact.
    assert_eq!(
        count_postings(&info_store, surviving_name).await,
        1,
        "surviving index postings intact"
    );
    // Dropped index's postings must be swept.
    assert_eq!(
        count_postings(&info_store, dropped_name).await,
        0,
        "dropped index postings swept by recovery"
    );
}
