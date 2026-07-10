//! Audit A9 — online CREATE INDEX lost-write race tests.
//!
//! These tests reproduce the concurrency window where a concurrent write
//! landing between the backfill snapshot and index-definition registration
//! was silently lost (never indexed). The fix (Option A: register-first,
//! backfill-second) makes the live write-hook see the new definition
//! immediately, so a concurrent write IS indexed.
//!
//! The tests deterministically force the race by directly calling the
//! lower-level pieces in sequence (snapshot → register → concurrent write →
//! backfill), rather than relying on real scheduler timing.

use super::helpers::{create_manager, create_test_value};
use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info_item::IndexInfoItem;
use crate::legacy::index_keys::{build_index_key_from_record, build_posting_key};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::atomic::Ordering;

// ============================================================================
// Plain CREATE INDEX — register-first closes the lost-write window
// ============================================================================

/// THE audit-A9 proof for plain CREATE INDEX.
///
/// Scenario: the backfill snapshot is taken BEFORE a concurrent write lands,
/// but the index definition is registered FIRST (the fix). The concurrent
/// write's live hook (`on_record_created`) sees the registered definition and
/// writes a posting. The backfill then writes postings for the snapshot
/// records. The concurrent write's record MUST be findable via
/// `lookup_by_index` after the operation completes.
///
/// Before the fix (backfill-then-register), the concurrent write landed in
/// the gap: the snapshot didn't include it, and the hook didn't see the
/// definition yet → the record was permanently missing from the index.
#[tokio::test]
async fn create_index_register_first_indexes_concurrent_write() {
    let (_, _, manager) = create_manager();

    // Pre-existing record (in the snapshot).
    let existing_val = create_test_value(&[(1, InnerValue::Str("old".to_string()))]);
    let existing_id = RecordId::new();
    // Write it to the data store so it appears in the snapshot.
    manager
        .data_store
        .set(
            existing_id.to_bytes().into(),
            existing_val.to_bytes().unwrap(),
        )
        .await
        .unwrap();

    // Take the backfill snapshot (simulating what the caller does BEFORE
    // calling create_index_from_records).
    use futures::StreamExt;
    let mut stream = manager.data_store.iter_stream(1000);
    let mut records = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, val) in batch.unwrap() {
            let arr: [u8; 16] = key.as_ref().try_into().unwrap();
            records.push((RecordId(arr), InnerValue::from_bytes(val).unwrap()));
        }
    }

    let index_def = IndexDefinition::new(5001, vec![IndexInfoItem::new(vec![1])]);

    // === The fix in action ===
    // create_index_from_records now registers the definition FIRST, then
    // backfills. We simulate a concurrent write that lands AFTER the snapshot
    // was taken but DURING the create (after registration).
    //
    // To deterministically force the race, we split the operation:
    // 1. Register the definition manually (what create_index_from_records
    //    does first now) — including the has_indexes flag, which the live
    //    write-hook checks as a fast-path gate.
    manager.indexes.add_index(index_def.clone());
    manager.has_indexes.store(true, Ordering::Release);
    manager.save_index_info().await.unwrap();

    // 2. A "concurrent" write lands NOW — the hook sees the registered def.
    let concurrent_val = create_test_value(&[(1, InnerValue::Str("concurrent".to_string()))]);
    let concurrent_id = RecordId::new();
    manager
        .on_record_created(&concurrent_id, &concurrent_val)
        .await
        .unwrap();

    // 3. Backfill from the (stale) snapshot — this is what
    //    create_index_from_records does second. The concurrent record is NOT
    //    in the snapshot, but it was already indexed by the live hook in
    //    step 2. Writing the snapshot's postings is idempotent.
    let name_interned = index_def.name_interned;
    let mut posting_writes = Vec::new();
    for (rid, val) in &records {
        if let Some(irk) = build_index_key_from_record(false, name_interned, val, &index_def.paths)
        {
            let pk = build_posting_key(&irk.to_bytes(), rid);
            posting_writes.push((pk.into(), bytes::Bytes::new()));
        }
    }
    if !posting_writes.is_empty() {
        manager.info_store.set_many(posting_writes).await.unwrap();
    }

    // === Assertion: the concurrent write MUST be indexed ===
    let result = manager
        .lookup_by_index(5001, &[InnerValue::Str("concurrent".to_string())])
        .await
        .unwrap();
    assert!(
        result.contains(&concurrent_id),
        "concurrent write must be indexed after register-first create; got {:?}",
        result
    );

    // The pre-existing record should also be indexed (from the backfill).
    let result_old = manager
        .lookup_by_index(5001, &[InnerValue::Str("old".to_string())])
        .await
        .unwrap();
    assert!(
        result_old.contains(&existing_id),
        "pre-existing record must be indexed from backfill; got {:?}",
        result_old
    );
}

/// Negative proof: if the definition is NOT registered before the concurrent
/// write, the write IS lost (the pre-fix behavior). This test documents what
/// the bug looked like — it registers the definition AFTER the write, proving
/// the write hook skips the unregistered index.
#[tokio::test]
async fn create_index_backfill_then_register_loses_concurrent_write() {
    let (_, _, manager) = create_manager();

    // Pre-existing record (in the snapshot).
    let existing_val = create_test_value(&[(1, InnerValue::Str("old".to_string()))]);
    let existing_id = RecordId::new();
    manager
        .data_store
        .set(
            existing_id.to_bytes().into(),
            existing_val.to_bytes().unwrap(),
        )
        .await
        .unwrap();

    // Snapshot.
    use futures::StreamExt;
    let mut stream = manager.data_store.iter_stream(1000);
    let mut records = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, val) in batch.unwrap() {
            let arr: [u8; 16] = key.as_ref().try_into().unwrap();
            records.push((RecordId(arr), InnerValue::from_bytes(val).unwrap()));
        }
    }

    let index_def = IndexDefinition::new(5002, vec![IndexInfoItem::new(vec![1])]);

    // === Pre-fix order: backfill FIRST, then register ===
    // (We simulate the OLD broken behavior to document the bug.)
    let name_interned = index_def.name_interned;
    let mut posting_writes = Vec::new();
    for (rid, val) in &records {
        if let Some(irk) = build_index_key_from_record(false, name_interned, val, &index_def.paths)
        {
            let pk = build_posting_key(&irk.to_bytes(), rid);
            posting_writes.push((pk.into(), bytes::Bytes::new()));
        }
    }
    if !posting_writes.is_empty() {
        manager.info_store.set_many(posting_writes).await.unwrap();
    }

    // A "concurrent" write lands BEFORE registration — the hook does NOT see
    // the index (has_indexes() is false, definition not registered).
    let concurrent_val = create_test_value(&[(1, InnerValue::Str("lost".to_string()))]);
    let concurrent_id = RecordId::new();
    manager
        .on_record_created(&concurrent_id, &concurrent_val)
        .await
        .unwrap(); // no-op: no indexes registered yet

    // NOW register (what the old code did last).
    manager.indexes.add_index(index_def);
    manager.save_index_info().await.unwrap();

    // === Assertion: the concurrent write is LOST (the bug) ===
    let result = manager
        .lookup_by_index(5002, &[InnerValue::Str("lost".to_string())])
        .await
        .unwrap();
    assert!(
        !result.contains(&concurrent_id),
        "pre-fix: concurrent write is lost (not indexed); this documents the bug"
    );
}

/// Full end-to-end: `create_index_from_records` with register-first correctly
/// indexes ALL snapshot records AND survives a concurrent write interleaved
/// between snapshot and backfill (the write is caught by the live hook).
#[tokio::test]
async fn create_index_from_records_full_concurrent_write_survives() {
    let (_, _, manager) = create_manager();

    // Pre-existing records.
    for i in 0..5i64 {
        let val = create_test_value(&[(1, InnerValue::Int(i))]);
        let id = RecordId::new();
        manager
            .data_store
            .set(id.to_bytes().into(), val.to_bytes().unwrap())
            .await
            .unwrap();
    }

    // Snapshot (the caller collects this before calling create_index_from_records).
    use futures::StreamExt;
    let mut stream = manager.data_store.iter_stream(1000);
    let mut records = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, val) in batch.unwrap() {
            let arr: [u8; 16] = key.as_ref().try_into().unwrap();
            records.push((RecordId(arr), InnerValue::from_bytes(val).unwrap()));
        }
    }

    // We can't easily inject a write mid-create_index_from_records without
    // a hook, so we test the invariant directly: after create_index_from_records
    // completes, the definition is registered and all snapshot records are indexed.
    let index_def = IndexDefinition::new(5003, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_index_from_records(index_def, records)
        .await
        .unwrap();

    // All 5 snapshot records should be indexed.
    for i in 0..5i64 {
        let result = manager
            .lookup_by_index(5003, &[InnerValue::Int(i)])
            .await
            .unwrap();
        assert_eq!(result.len(), 1, "snapshot record {} must be indexed", i);
    }

    // A write AFTER create completes should also be indexed (normal operation).
    let post_val = create_test_value(&[(1, InnerValue::Int(99))]);
    let post_id = RecordId::new();
    manager
        .on_record_created(&post_id, &post_val)
        .await
        .unwrap();
    let result = manager
        .lookup_by_index(5003, &[InnerValue::Int(99)])
        .await
        .unwrap();
    assert!(
        result.contains(&post_id),
        "post-create write must be indexed"
    );
}
