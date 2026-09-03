//! P0-3b (#988): DROP INDEX durable tombstone + crash recovery tests for the
//! index2 family (fts / functional / vector).
//!
//! Mirrors #972's `p03b_sorted_drop_durability_tests.rs` one-to-one for
//! index2. Tests the three sub-bugs:
//! - **3c** (crash-resurrection): a crash between sweep and metadata-persist
//!   must NOT resurrect a fully-broken "Ready but no postings" index.
//! - **3c** (idempotent resume): calling the recovery path twice must be a
//!   clean no-op on the second call.
//! - **3b** (name-reuse ghost postings): a `create_index_v2` reusing a name
//!   whose DROP is still in flight must be rejected until the tombstone clears.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::persistence;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;

// ============================================================================
// Helpers
// ============================================================================

/// Fresh in-memory stores so that dropping a `TableManager` does NOT lose
/// data — the reopen sees the same on-disk bytes the crashed process wrote.
fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// A functional `lower(<field>)` index create op.
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

/// Count posting entries under one index2 id's prefix (4 bytes LE).
async fn count_postings(info_store: &Arc<dyn Store>, id: u32) -> usize {
    let prefix = Bytes::copy_from_slice(&id.to_le_bytes());
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

/// Remove every posting under one index2 id's prefix directly from the store
/// — used to simulate the DROP's sweep step having ALREADY run before the
/// simulated crash.
async fn sweep_postings_direct(info_store: &Arc<dyn Store>, id: u32) {
    let prefix = Bytes::copy_from_slice(&id.to_le_bytes());
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
/// after `add_to_dropping_index2` but before the sweep/persist completes.
///
/// #1204: routed through the real `add_to_dropping_index2` so this fixture
/// always matches whatever wire format that function actually writes,
/// instead of hand-rolling a raw `bincode::serialize` of a bare `Vec<u32>`
/// (a shape `add_to_dropping_index2` has not written since #1051, and which
/// would no longer decode at all once #1204's version-byte envelope landed).
async fn seed_tombstone(info_store: &Arc<dyn Store>, ids: &[u32]) {
    for &id in ids {
        persistence::add_to_dropping_index2(id, String::new(), None, info_store)
            .await
            .unwrap();
    }
}

/// Read back the persisted tombstone (empty vec if absent). #1204: routed
/// through the real `load_dropping_index2` rather than a local raw
/// `bincode::deserialize::<Vec<u32>>`, which stopped matching the on-disk
/// shape after #1051 (and again after #1204's version-byte envelope).
async fn load_tombstone(info_store: &Arc<dyn Store>) -> Vec<(u32, String, Option<String>)> {
    persistence::load_dropping_index2(info_store).await.unwrap()
}

/// Create a table with a functional index + data, persist the interner, then
/// drop the manager. Returns (data_store, info_store, index_id).
async fn seed_table_with_index(
    index_name: &str,
    values: &[&str],
) -> (Arc<dyn Store>, Arc<dyn Store>, u32) {
    let (data_store, info_store) = make_stores();

    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let name_field = key_id(&mgr, "name").await;
    for &val in values {
        mgr.insert(&record_with_str(name_field, val)).await.unwrap();
    }
    // Persist the interner so interned field-name ids survive reopen.
    mgr.interner().persist().await.unwrap();

    mgr.create_index_v2(&functional_lower_op(index_name, "people", "name"))
        .await
        .unwrap();

    // Look up the assigned descriptor id.
    let name_interned = key_id(&mgr, index_name).await;
    let backend = mgr
        .index2_registry()
        .get_by_name(name_interned)
        .await
        .expect("index must be registered");
    let id = backend.descriptor().id;

    // Drop the manager — the in-memory state dies, but the info_store and
    // data_store are shared Arcs so the persisted bytes survive.
    drop(mgr);

    (data_store, info_store, id)
}

// ============================================================================
// 3c — crash-resurrection: crash AFTER tombstone-write, BEFORE sweep
// ============================================================================

#[tokio::test]
async fn p03b_index2_crash_before_sweep_postings_still_present() {
    let (data_store, info_store, id) = seed_table_with_index("lower_name", &["Alice", "Bob"]).await;
    assert!(
        count_postings(&info_store, id).await > 0,
        "precondition: postings present after create"
    );

    // Crash state: tombstone written, postings NOT swept, persisted metadata
    // still lists the index as Ready.
    seed_tombstone(&info_store, &[id]).await;

    // Reopen — recovery MUST run.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    // The index must NOT be visible (no resurrection).
    let backends = mgr.index2_registry().all_backends().await;
    assert!(
        backends.is_empty(),
        "3c FAIL: dropped index2 backend was resurrected after crash"
    );

    // Postings must be swept by recovery.
    assert_eq!(
        count_postings(&info_store, id).await,
        0,
        "3c FAIL: recovery must sweep postings"
    );

    // Tombstone must be cleared.
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c FAIL: tombstone should be cleared after recovery"
    );
}

// ============================================================================
// 3c — crash-resurrection: crash AFTER sweep, BEFORE persist
// ============================================================================

#[tokio::test]
async fn p03b_index2_crash_after_sweep_before_persist() {
    let (data_store, info_store, id) = seed_table_with_index("lower_name", &["Alice", "Bob"]).await;
    assert!(
        count_postings(&info_store, id).await > 0,
        "precondition: postings present after create"
    );

    // Simulate the DROP's sweep step having already run.
    sweep_postings_direct(&info_store, id).await;
    assert_eq!(count_postings(&info_store, id).await, 0);

    // Tombstone written, reduced metadata NOT yet persisted.
    seed_tombstone(&info_store, &[id]).await;

    // Reopen — recovery MUST run.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.index2_registry().all_backends().await.is_empty(),
        "3c FAIL: dropped index2 backend was resurrected after crash"
    );
    assert_eq!(
        count_postings(&info_store, id).await,
        0,
        "3c FAIL: postings must stay swept"
    );
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c FAIL: tombstone should be cleared after recovery"
    );
}

// ============================================================================
// 3c — crash-resurrection: crash AFTER persist, BEFORE tombstone clear
// ============================================================================

#[tokio::test]
async fn p03b_index2_crash_after_persist_before_tombstone_clear() {
    let (data_store, info_store, id) = seed_table_with_index("lower_name", &["Alice", "Bob"]).await;

    // Simulate: sweep ran, metadata persisted (index removed), tombstone
    // still present.
    sweep_postings_direct(&info_store, id).await;
    // Overwrite persisted metadata with an empty registry (index removed).
    let empty_registry = crate::index2::IndexRegistry::new();
    persistence::save_index2_metadata(&empty_registry, &info_store)
        .await
        .unwrap();
    // Tombstone still present.
    seed_tombstone(&info_store, &[id]).await;

    // Reopen — recovery should just clear the stale tombstone.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        mgr.index2_registry().all_backends().await.is_empty(),
        "3c: index gone (not in persisted metadata before crash)"
    );
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c: stale tombstone cleared after recovery"
    );
}

// ============================================================================
// 3c — idempotent resume (two restart attempts)
// ============================================================================

#[tokio::test]
async fn p03b_index2_idempotent_resume_double_restart() {
    let (data_store, info_store, id) = seed_table_with_index("lower_name", &["Alice"]).await;

    // Sweep + tombstone (crash state: sweep ran, persist did not).
    sweep_postings_direct(&info_store, id).await;
    seed_tombstone(&info_store, &[id]).await;

    // First restart — recovery runs.
    let mgr1 = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    assert!(mgr1.index2_registry().all_backends().await.is_empty());

    // Second restart — must be a clean no-op, not an error.
    let mgr2 = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(
        mgr2.index2_registry().all_backends().await.is_empty(),
        "3c idempotent: no backends after second restart"
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
async fn p03b_index2_live_drop_crash_at_post_sweep_hook() {
    let (data_store, info_store) = make_stores();

    // Create and populate the index via the real path.
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
    mgr.insert(&record_with_str(name_field, "Bob"))
        .await
        .unwrap();
    mgr.interner().persist().await.unwrap();
    mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    let name_interned = key_id(&mgr, "lower_name").await;
    let backend = mgr
        .index2_registry()
        .get_by_name(name_interned)
        .await
        .expect("index must be registered");
    let id = backend.descriptor().id;
    assert!(
        count_postings(&info_store, id).await > 0,
        "precondition: postings present after create"
    );

    // Install the post-sweep hook.
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.set_drop_index2_post_sweep_hook(Some(Arc::clone(&hook)));

    // Start drop and let it park at the post-sweep hook (sweep done,
    // reduced metadata NOT yet persisted). Using `select!` so the losing
    // branch (the drop future) is cancelled, simulating a crash.
    let mgr_c = mgr.clone();
    tokio::select! {
        _ = mgr_c.drop_index2("lower_name", None) => {
            panic!("drop_index2 completed before post-sweep hook fired");
        }
        _ = hook.wait_until_parked() => {
            // Parked: sweep done, metadata not yet persisted.
        }
    }

    // Simulate crash: drop the manager (its in-memory state dies). The select
    // already cancelled the drop_index2 future.
    drop(mgr_c);
    drop(mgr);

    // Reopen — recovery MUST finish the drop.
    let new_mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        new_mgr.index2_registry().all_backends().await.is_empty(),
        "3c LIVE: dropped index2 backend must not be resurrected after simulated crash"
    );
    assert_eq!(
        count_postings(&info_store, id).await,
        0,
        "3c LIVE: postings swept by recovery"
    );
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "3c LIVE: tombstone cleared after recovery"
    );
}

// ============================================================================
// 3b — name-reuse rejection during in-flight DROP
// ============================================================================

#[tokio::test]
async fn p03b_index2_name_reuse_rejected_during_drop() {
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

    // Install the pre-sweep pause hook (fires after tombstone-write + retire,
    // before the sweep).
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.set_drop_index2_pause_hook(Some(Arc::clone(&hook)));

    // Start drop in a background task.
    let mgr_c = mgr.clone();
    let drop_task = tokio::spawn(async move {
        mgr_c.drop_index2("lower_name", None).await.unwrap();
    });

    // Wait for the drop to park (tombstone written, backend retired, pre-sweep).
    hook.wait_until_parked().await;

    // Attempt create with the same name — MUST be rejected.
    let create_result = mgr
        .create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await;
    assert!(
        create_result.is_err(),
        "3b FAIL: create_index_v2 during in-flight DROP must be rejected, got: {:?}",
        create_result
    );

    // Release the hook and let DROP complete.
    hook.release();
    drop_task.await.unwrap();

    // After DROP completes (tombstone cleared), create with the same name
    // should succeed.
    let create_result = mgr
        .create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await;
    assert!(
        create_result.is_ok(),
        "3b: create_index_v2 after DROP completes must succeed, got: {:?}",
        create_result.err()
    );
}

// ============================================================================
// Normal DROP still works (smoke test — no regression from tombstone changes)
// ============================================================================

#[tokio::test]
async fn p03b_index2_normal_drop_still_works() {
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

    let name_interned = key_id(&mgr, "lower_name").await;
    let backend = mgr
        .index2_registry()
        .get_by_name(name_interned)
        .await
        .expect("index must be registered");
    let id = backend.descriptor().id;
    assert!(count_postings(&info_store, id).await > 0);

    let removed = mgr.drop_index2("lower_name", None).await.unwrap();
    assert!(removed);
    assert!(mgr.index2_registry().all_backends().await.is_empty());
    assert_eq!(
        count_postings(&info_store, id).await,
        0,
        "postings swept after normal drop"
    );

    // Tombstone must be cleared after a normal drop.
    assert!(
        load_tombstone(&info_store).await.is_empty(),
        "tombstone must not contain the dropped id after normal drop"
    );

    // A fresh manager must load clean state (no resurrection).
    drop(mgr);
    let mgr2 = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr2.index2_registry().all_backends().await.is_empty());
}
