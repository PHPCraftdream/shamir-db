//! P0-3 (#959): DROP INDEX durable tombstone + crash recovery tests.
//!
//! Tests the three sub-bugs:
//! - **3c** (crash-resurrection): a crash between sweep and metadata-persist
//!   must NOT resurrect a fully-broken "Ready but no postings" index.
//! - **3c** (idempotent resume): calling the recovery path twice must be a
//!   clean no-op on the second call.
//! - **3b** (name-reuse ghost postings): a CREATE INDEX reusing a name whose
//!   DROP is still in flight must be rejected until the tombstone clears.

use crate::base_index::backfill_pause_hook::BackfillPauseHook;
use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info::IndexInfo;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_keys::{build_index_key_from_record, build_posting_key};
use crate::base_index::index_manager::IndexManager;
use crate::base_index::index_record_key::IndexRecordKey;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

// ============================================================================
// Helpers
// ============================================================================

/// Create a fresh IndexManager backed by in-memory stores.
fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    (data_store, info_store)
}

/// Build a simple test InnerValue::Map with one field.
fn make_value(field_key: u64, field_val: &str) -> InnerValue {
    let mut map = new_map();
    map.insert(
        shamir_types::core::interner::InternerKey::new(field_key),
        InnerValue::Str(field_val.to_string()),
    );
    InnerValue::Map(map)
}

/// Write a regular index definition + its postings directly into info_store,
/// simulating a fully-built, persisted Ready index. Returns the name_interned.
async fn seed_regular_index(
    info_store: &Arc<dyn Store>,
    name_interned: u64,
    field_key: u64,
    values: &[&str],
) {
    // Persist the IndexInfo with the definition at Ready.
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![field_key])]);
    let info = IndexInfo::from_definitions(vec![def]);
    let key = RecordId::system("indexes").to_bytes();
    let bytes = bincode::serialize(&info).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();

    // Write posting entries under the index's prefix.
    let mut entries: Vec<(RecordKey, Bytes)> = Vec::new();
    for (i, val) in values.iter().enumerate() {
        let v = make_value(field_key, val);
        let irk = build_index_key_from_record(
            false,
            name_interned,
            &v,
            &[IndexInfoItem::new(vec![field_key])],
        )
        .unwrap();
        let index_key = irk.to_bytes();
        let record_id = RecordId::new();
        let posting_key = build_posting_key(&index_key, &record_id);
        entries.push((posting_key.into(), Bytes::new()));
        // Also write the data record so it looks realistic.
        let _ = i; // suppress unused warning
    }
    if !entries.is_empty() {
        info_store.set_many(entries).await.unwrap();
    }
}

/// Write a unique index definition + its postings directly into info_store.
async fn seed_unique_index(
    info_store: &Arc<dyn Store>,
    name_interned: u64,
    field_key: u64,
    values: &[&str],
) {
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![field_key])]);
    let info = IndexInfo::from_definitions(vec![def]);
    let key = RecordId::system("indexes_unique").to_bytes();
    let bytes = bincode::serialize(&info).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();

    // Write unique posting entries (index_key → record_id bytes).
    let mut entries: Vec<(RecordKey, Bytes)> = Vec::new();
    for val in values {
        let v = make_value(field_key, val);
        let irk = build_index_key_from_record(
            true,
            name_interned,
            &v,
            &[IndexInfoItem::new(vec![field_key])],
        )
        .unwrap();
        let index_key = irk.to_bytes();
        let record_id = RecordId::new();
        entries.push((
            index_key.into(),
            Bytes::copy_from_slice(record_id.as_bytes()),
        ));
    }
    if !entries.is_empty() {
        info_store.set_many(entries).await.unwrap();
    }
}

/// Write a tombstone directly into info_store, simulating the persisted state
/// after `add_to_dropping` but before the sweep/persist completes.
async fn seed_tombstone(info_store: &Arc<dyn Store>, is_unique: bool, names: &[u64]) {
    let key_str = if is_unique { "uidx_drop" } else { "idx_drop" };
    let key = RecordId::system(key_str).to_bytes();
    let bytes = bincode::serialize(names).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
}

/// Count how many posting keys exist under a given index prefix.
async fn count_postings(info_store: &Arc<dyn Store>, is_unique: bool, name_interned: u64) -> usize {
    use futures::StreamExt;
    let prefix = IndexRecordKey::new(is_unique, name_interned).to_prefix_bytes();
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

// ============================================================================
// 3c — crash-resurrection recovery (direct state setup)
// ============================================================================

#[tokio::test]
async fn p03_3c_regular_crash_after_sweep_does_not_resurrect() {
    // Simulate: index was Ready with postings, DROP started (tombstone written,
    // postings swept), but the process crashed before the reduced IndexInfo
    // was persisted. The on-disk state: old IndexInfo (def still Ready),
    // tombstone present, postings gone.
    let (data_store, info_store) = make_stores();
    let name_interned = 5001u64;

    // Seed a Ready index with postings.
    seed_regular_index(&info_store, name_interned, 1, &["alice", "bob", "carol"]).await;
    assert_eq!(
        count_postings(&info_store, false, name_interned).await,
        3,
        "precondition: 3 postings seeded"
    );

    // Sweep the postings (simulating the DROP's sweep step that already ran).
    // We do this directly because the test is about the RECOVERY path, not
    // the DROP itself.
    {
        use futures::StreamExt;
        let prefix = IndexRecordKey::new(false, name_interned).to_prefix_bytes();
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

    // Write the tombstone (simulating add_to_dropping).
    seed_tombstone(&info_store, false, &[name_interned]).await;

    // Verify crash state: def is in IndexInfo, postings are gone, tombstone present.
    assert_eq!(count_postings(&info_store, false, name_interned).await, 0);

    // Construct a fresh IndexManager — recovery MUST run.
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    // The index must NOT be visible as Ready (or at all).
    assert!(
        !manager.index_exists(name_interned),
        "3c FAIL: dropped index was resurrected as Ready after crash"
    );
    assert!(
        !manager.has_indexes(),
        "3c FAIL: has_indexes should be false after recovery"
    );

    // The on-disk IndexInfo must now be the reduced (empty) form.
    let indexes_key = RecordId::system("indexes").to_bytes();
    let bytes = info_store.get(indexes_key.into()).await.unwrap();
    let info = IndexInfo::decode_bytes(&bytes).unwrap();
    assert!(
        info.is_empty(),
        "3c FAIL: reduced IndexInfo should have no definitions"
    );

    // Tombstone must be cleared.
    let tomb_key = RecordId::system("idx_drop").to_bytes();
    let tomb_bytes = info_store.get(tomb_key.into()).await.unwrap();
    let tomb: Vec<u64> = bincode::deserialize(&tomb_bytes).unwrap();
    assert!(
        tomb.is_empty(),
        "3c FAIL: tombstone should be cleared after recovery"
    );
}

#[tokio::test]
async fn p03_3c_unique_crash_after_sweep_does_not_resurrect() {
    let (data_store, info_store) = make_stores();
    let name_interned = 6001u64;

    seed_unique_index(&info_store, name_interned, 1, &["alice", "bob"]).await;
    assert_eq!(
        count_postings(&info_store, true, name_interned).await,
        2,
        "precondition: 2 unique postings seeded"
    );

    // Sweep postings (simulating completed sweep).
    {
        use futures::StreamExt;
        let prefix = IndexRecordKey::new(true, name_interned).to_prefix_bytes();
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

    // Write tombstone.
    seed_tombstone(&info_store, true, &[name_interned]).await;

    // Construct fresh manager — recovery MUST run.
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !manager.unique_index_exists(name_interned),
        "3c FAIL: dropped unique index was resurrected after crash"
    );
    assert!(
        !manager.has_unique_indexes(),
        "3c FAIL: has_unique_indexes should be false after recovery"
    );
}

// ============================================================================
// 3c — idempotent resume (two restart attempts)
// ============================================================================

#[tokio::test]
async fn p03_3c_idempotent_resume_double_restart() {
    let (data_store, info_store) = make_stores();
    let name_interned = 7001u64;

    seed_regular_index(&info_store, name_interned, 1, &["x", "y"]).await;

    // Sweep + tombstone (crash state).
    {
        use futures::StreamExt;
        let prefix = IndexRecordKey::new(false, name_interned).to_prefix_bytes();
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
    seed_tombstone(&info_store, false, &[name_interned]).await;

    // First restart — recovery runs.
    let manager1 = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(!manager1.index_exists(name_interned));

    // Second restart — must be a clean no-op, not an error or double-sweep.
    let manager2 = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(
        !manager2.index_exists(name_interned),
        "3c idempotent: index still gone after second restart"
    );
    assert!(
        !manager2.has_indexes(),
        "3c idempotent: has_indexes false after second restart"
    );

    // Tombstone must be empty (cleared by first recovery, absent for second).
    let tomb_key = RecordId::system("idx_drop").to_bytes();
    let tomb_bytes = info_store.get(tomb_key.into()).await.unwrap();
    let tomb: Vec<u64> = bincode::deserialize(&tomb_bytes).unwrap();
    assert!(
        tomb.is_empty(),
        "3c idempotent: tombstone empty after second restart"
    );
}

// ============================================================================
// 3c — crash between tombstone-write and sweep (postings still present)
// ============================================================================

#[tokio::test]
async fn p03_3c_regular_crash_before_sweep_postings_still_present() {
    // Simulate: DROP started (tombstone written), but crashed BEFORE the
    // sweep. Postings are still intact. Recovery must sweep + remove def.
    let (data_store, info_store) = make_stores();
    let name_interned = 8001u64;

    seed_regular_index(&info_store, name_interned, 1, &["a", "b"]).await;
    assert_eq!(
        count_postings(&info_store, false, name_interned).await,
        2,
        "precondition: 2 postings seeded"
    );

    // Tombstone only — do NOT sweep (simulating crash between tombstone and sweep).
    seed_tombstone(&info_store, false, &[name_interned]).await;

    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !manager.index_exists(name_interned),
        "3c FAIL: index should be gone after recovery swept it"
    );

    // Postings must be swept by recovery.
    assert_eq!(
        count_postings(&info_store, false, name_interned).await,
        0,
        "3c FAIL: recovery must sweep postings"
    );
}

// ============================================================================
// 3c — crash after persist but before tombstone clear
// ============================================================================

#[tokio::test]
async fn p03_3c_regular_crash_after_persist_before_tombstone_clear() {
    // Simulate: DROP fully completed (def removed from IndexInfo, postings
    // swept, reduced IndexInfo persisted), but crashed before tombstone was
    // cleared. Recovery should just clear the stale tombstone.
    let (data_store, info_store) = make_stores();
    let name_interned = 9001u64;

    // Seed index, then simulate completed DROP:
    // - Reduced (empty) IndexInfo persisted
    // - Postings swept (gone)
    // - Tombstone still present
    let empty_info = IndexInfo::new();
    let indexes_key = RecordId::system("indexes").to_bytes();
    let bytes = bincode::serialize(&empty_info).unwrap();
    info_store
        .set(indexes_key.into(), bytes.into())
        .await
        .unwrap();
    seed_tombstone(&info_store, false, &[name_interned]).await;

    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !manager.index_exists(name_interned),
        "3c: index gone (def already removed from IndexInfo before crash)"
    );

    // Tombstone must be cleared by recovery.
    let tomb_key = RecordId::system("idx_drop").to_bytes();
    let tomb_bytes = info_store.get(tomb_key.into()).await.unwrap();
    let tomb: Vec<u64> = bincode::deserialize(&tomb_bytes).unwrap();
    assert!(
        tomb.is_empty(),
        "3c: stale tombstone cleared after recovery"
    );
}

// ============================================================================
// 3c — live DROP with post-sweep hook: simulates real crash mid-operation
// ============================================================================

#[tokio::test]
async fn p03_3c_live_drop_crash_at_post_sweep_hook_regular() {
    let (data_store, info_store) = make_stores();
    let name_interned = 10001u64;

    // Create and populate the index via the real create path.
    {
        let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
            .await
            .unwrap();
        let records = vec![
            (RecordId::new(), make_value(1, "alice")),
            (RecordId::new(), make_value(1, "bob")),
        ];
        let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]);
        manager
            .create_index_from_records(def, records)
            .await
            .unwrap();
        assert!(manager.index_exists(name_interned));
        assert!(manager.has_indexes());
    }
    // The first manager is dropped here — its in-memory state dies but the
    // on-disk IndexInfo (with the Ready def) persists in info_store.
    assert_eq!(
        count_postings(&info_store, false, name_interned).await,
        2,
        "precondition: 2 postings after create"
    );

    // Create a second manager and install the post-sweep hook.
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    let hook = Arc::new(BackfillPauseHook::new());
    manager.set_drop_index_post_sweep_hook(Some(Arc::clone(&hook)));

    // Start drop_index and let it park at the post-sweep hook.
    let mgr = manager.clone();
    tokio::select! {
        _ = mgr.drop_index(name_interned, None) => {
            panic!("drop_index completed before post-sweep hook fired");
        }
        _ = hook.wait_until_parked() => {
            // Parked: sweep done, IndexInfo not yet persisted.
        }
    }

    // Simulate crash: drop the manager (its in-memory state dies).
    // The select already cancelled the drop_index future.
    drop(mgr);
    drop(manager);

    // Construct a fresh manager — recovery MUST finish the drop.
    let new_manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !new_manager.index_exists(name_interned),
        "3c LIVE: dropped index must not be resurrected after simulated crash"
    );
    assert!(
        !new_manager.has_indexes(),
        "3c LIVE: has_indexes must be false after recovery"
    );
}

// ============================================================================
// 3b — name-reuse rejection during in-flight DROP
// ============================================================================

#[tokio::test]
async fn p03_3b_regular_name_reuse_rejected_during_drop() {
    let (data_store, info_store) = make_stores();
    let name_interned = 11001u64;

    // Create and populate the index.
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    let records = vec![
        (RecordId::new(), make_value(1, "alice")),
        (RecordId::new(), make_value(1, "bob")),
    ];
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_index_from_records(def, records)
        .await
        .unwrap();

    // Install the drop pause hook (fires between tombstone-write and sweep).
    let hook = Arc::new(BackfillPauseHook::new());
    manager.set_drop_index_pause_hook(Some(Arc::clone(&hook)));

    // Start drop in a background task.
    let mgr = manager.clone();
    let drop_task = tokio::spawn(async move {
        mgr.drop_index(name_interned, None).await.unwrap();
    });

    // Wait for the drop to park (tombstone written, def retired, pre-sweep).
    hook.wait_until_parked().await;

    // Attempt CREATE with the same name — MUST be rejected.
    let create_result = manager
        .create_index_from_records(
            IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]),
            Vec::new(),
        )
        .await;
    assert!(
        create_result.is_err(),
        "3b FAIL: CREATE INDEX during in-flight DROP must be rejected"
    );

    // Release the hook and let DROP complete.
    hook.release();
    drop_task.await.unwrap();

    // After DROP completes (tombstone cleared), CREATE with the same name
    // should succeed.
    let create_result = manager
        .create_index_from_records(
            IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]),
            Vec::new(),
        )
        .await;
    assert!(
        create_result.is_ok(),
        "3b: CREATE INDEX after DROP completes must succeed, got: {:?}",
        create_result.err()
    );
    assert!(manager.index_exists(name_interned));
}

#[tokio::test]
async fn p03_3b_unique_name_reuse_rejected_during_drop() {
    let (data_store, info_store) = make_stores();
    let name_interned = 12001u64;

    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    let records = vec![
        (RecordId::new(), make_value(1, "alice")),
        (RecordId::new(), make_value(1, "bob")),
    ];
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_unique_index_from_records(def, records)
        .await
        .unwrap();

    let hook = Arc::new(BackfillPauseHook::new());
    manager.set_drop_index_pause_hook(Some(Arc::clone(&hook)));

    let mgr = manager.clone();
    let drop_task = tokio::spawn(async move {
        mgr.drop_unique_index(name_interned, None).await.unwrap();
    });

    hook.wait_until_parked().await;

    // Attempt CREATE UNIQUE with the same name — MUST be rejected.
    let create_result = manager
        .create_unique_index_from_records(
            IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]),
            Vec::new(),
        )
        .await;
    assert!(
        create_result.is_err(),
        "3b FAIL: CREATE UNIQUE INDEX during in-flight DROP must be rejected"
    );

    hook.release();
    drop_task.await.unwrap();

    // After DROP completes, CREATE should succeed.
    let create_result = manager
        .create_unique_index_from_records(
            IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]),
            Vec::new(),
        )
        .await;
    assert!(
        create_result.is_ok(),
        "3b: CREATE UNIQUE INDEX after DROP completes must succeed"
    );
    assert!(manager.unique_index_exists(name_interned));
}

// ============================================================================
// Normal DROP still works (smoke test — no regression from tombstone changes)
// ============================================================================

#[tokio::test]
async fn p03_normal_drop_regular_still_works() {
    let (data_store, info_store) = make_stores();
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    let name_interned = 13001u64;

    let records = vec![
        (RecordId::new(), make_value(1, "alice")),
        (RecordId::new(), make_value(1, "bob")),
    ];
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_index_from_records(def, records)
        .await
        .unwrap();
    assert!(manager.index_exists(name_interned));
    assert_eq!(count_postings(&info_store, false, name_interned).await, 2);

    let removed = manager.drop_index(name_interned, None).await.unwrap();
    assert!(removed);
    assert!(!manager.index_exists(name_interned));
    assert!(!manager.has_indexes());
    assert_eq!(
        count_postings(&info_store, false, name_interned).await,
        0,
        "postings swept after normal drop"
    );

    // Tombstone must be cleared after a normal drop.
    let tomb_key = RecordId::system("idx_drop").to_bytes();
    match info_store.get(tomb_key.into()).await {
        Ok(bytes) => {
            let tomb: Vec<u64> = bincode::deserialize(&bytes).unwrap();
            assert!(
                !tomb.contains(&name_interned),
                "tombstone must not contain the dropped name after normal drop"
            );
        }
        Err(_) => { /* key absent — also fine */ }
    }

    // A fresh manager must load clean state (no resurrection).
    let manager2 = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(!manager2.index_exists(name_interned));
}

#[tokio::test]
async fn p03_normal_drop_unique_still_works() {
    let (data_store, info_store) = make_stores();
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    let name_interned = 14001u64;

    let records = vec![
        (RecordId::new(), make_value(1, "alice")),
        (RecordId::new(), make_value(1, "bob")),
    ];
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_unique_index_from_records(def, records)
        .await
        .unwrap();
    assert!(manager.unique_index_exists(name_interned));

    let removed = manager
        .drop_unique_index(name_interned, None)
        .await
        .unwrap();
    assert!(removed);
    assert!(!manager.unique_index_exists(name_interned));
    assert!(!manager.has_unique_indexes());

    let manager2 = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(!manager2.unique_index_exists(name_interned));
}

// ============================================================================
// Surviving indexes are NOT affected by recovery of a different name
// ============================================================================

#[tokio::test]
async fn p03_3c_recovery_does_not_affect_surviving_indexes() {
    let (data_store, info_store) = make_stores();
    let dropped_name = 15001u64;
    let surviving_name = 15002u64;

    // Seed two regular indexes.
    seed_regular_index(&info_store, dropped_name, 1, &["a"]).await;
    seed_regular_index(&info_store, surviving_name, 2, &["b"]).await;

    // Write BOTH into the same IndexInfo.
    let info = IndexInfo::from_definitions(vec![
        IndexDefinition::new(dropped_name, vec![IndexInfoItem::new(vec![1])]),
        IndexDefinition::new(surviving_name, vec![IndexInfoItem::new(vec![2])]),
    ]);
    let key = RecordId::system("indexes").to_bytes();
    let bytes = bincode::serialize(&info).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();

    // Tombstone only the dropped one.
    seed_tombstone(&info_store, false, &[dropped_name]).await;

    // Construct fresh manager — recovery must drop only `dropped_name`.
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !manager.index_exists(dropped_name),
        "dropped index should be gone"
    );
    assert!(
        manager.index_exists(surviving_name),
        "surviving index must NOT be affected by recovery"
    );
    assert!(
        manager.has_indexes(),
        "has_indexes must be true (surviving index present)"
    );

    // Surviving index's postings must be intact.
    assert_eq!(
        count_postings(&info_store, false, surviving_name).await,
        1,
        "surviving index postings intact"
    );
}
