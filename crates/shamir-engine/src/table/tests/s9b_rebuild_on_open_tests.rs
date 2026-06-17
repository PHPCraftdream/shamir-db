//! S9b (#81): rebuild-on-open for legacy index format v2.
//!
//! Proves that when a table is reopened with an outdated legacy-index format
//! marker (version < 2), `TableManager::create` triggers `repair()` and
//! stamps version 2, leaving every index consistent.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::index::index_record_key::IndexRecordKey;
use crate::index2::persistence::load_legacy_index_version;
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::TableManager;

/// Build shared in-memory stores, open a fresh table on them, create a
/// regular index, seed a few records, and return (table, data_store, info_store).
/// After this call the table is stamped with legacy_index_version == 2 (by
/// the create() trigger on its first open).
async fn fresh_table_with_index() -> (TableManager, Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let t = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    t.create_index("by_x", &["x"]).await.unwrap();

    let interner = t.interner().get().await.unwrap();
    for i in 0..5u32 {
        let mut m = new_map();
        m.insert("x".to_string(), QueryValue::Int(i as i64));
        let qv = QueryValue::Map(m);
        let (iv, new_keys) = query_value_to_inner_tracked(&qv, interner).unwrap();
        if !new_keys.is_empty() {
            t.interner().save_new_keys(&new_keys).await.unwrap();
        }
        t.insert(&iv).await.unwrap();
    }
    // Persist the record counter (incremented only in-memory during insert)
    // so reopening on the same stores sees the correct count.
    t.flush_metadata().await.unwrap();

    (t, data, info)
}

#[tokio::test]
async fn s9b_outdated_marker_triggers_rebuild_on_open() {
    // -----------------------------------------------------------------------
    // 1. Create a table, add an index, seed data.
    // -----------------------------------------------------------------------
    let (t1, data, info) = fresh_table_with_index().await;

    // Baseline: healthy and already stamped at 2 by the first create().
    let report = t1.verify().await.unwrap();
    assert!(
        report.is_healthy(),
        "fresh table must be healthy, got {report:?}"
    );
    let v = load_legacy_index_version(&info).await.unwrap();
    assert_eq!(v, 2, "first open must stamp version 2");

    // -----------------------------------------------------------------------
    // 2. Simulate an outdated on-disk format: lower the marker to 1.
    //
    //    The key mirrors the private `meta_key_legacy_index_version()` from
    //    shamir_index::persistence — RecordId::system("_m.idx.lfv").
    //    We duplicate it here intentionally (with this comment) rather than
    //    making it public, because the test is the only caller and the
    //    coupling is explicit.
    // -----------------------------------------------------------------------
    let marker_key = RecordId::system("_m.idx.lfv").to_bytes();
    info.set(marker_key, Bytes::from(1u32.to_le_bytes().to_vec()))
        .await
        .unwrap();

    assert_eq!(
        load_legacy_index_version(&info).await.unwrap(),
        1,
        "marker must be 1 after manual downgrade"
    );

    // -----------------------------------------------------------------------
    // 3. Plant an orphan posting so repair has an observable effect.
    //
    //    IndexRecordKey::new(false, def.name_interned) is the standard
    //    posting-list prefix for a regular index; appending a random RecordId
    //    produces a posting key with no corresponding data record.
    // -----------------------------------------------------------------------
    let regular_defs: Vec<_> = t1.index_manager_ref().iter_indexes().collect();
    let def = regular_defs.first().expect("by_x index must exist");

    let prefix = IndexRecordKey::new(false, def.name_interned).to_bytes();
    let fake_rid = RecordId::new();
    let mut orphan_key = prefix.to_vec();
    orphan_key.extend_from_slice(fake_rid.as_bytes());
    info.set(Bytes::from(orphan_key), Bytes::new())
        .await
        .unwrap();

    // Before reopening: verify shows the orphan.
    let before = t1.verify().await.unwrap();
    assert!(
        !before.is_healthy(),
        "orphan posting must make the table unhealthy before reopen"
    );

    drop(t1);

    // -----------------------------------------------------------------------
    // 4. Reopen on the SAME stores — the trigger fires:
    //      legacy_indexes_need_rebuild() → true (stored == 1 < 2)
    //      has_legacy == true (by_x exists)
    //      repair() is called → orphan is cleared, postings are rebuilt
    //      save_legacy_index_version() stamps 2
    // -----------------------------------------------------------------------
    let t2 = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    // -----------------------------------------------------------------------
    // 5. Key assertions — non-vacuous because the marker was 1 before reopen.
    // -----------------------------------------------------------------------
    let stamped = load_legacy_index_version(&info).await.unwrap();
    assert_eq!(
        stamped, 2,
        "reopen must stamp legacy_index_version = 2 (was 1 before)"
    );

    let after = t2.verify().await.unwrap();
    assert!(
        after.is_healthy(),
        "repair on reopen must remove the orphan and leave the table healthy: {after:?}"
    );

    // All 5 records must still be present and indexed.
    assert_eq!(after.records_in_data, 5, "record count must be preserved");
    assert_eq!(after.counter_value, 5, "counter must be recounted to 5");
    for ih in &after.regular_indexes {
        assert_eq!(
            ih.expected_entries, ih.actual_entries,
            "index {}: expected {} actual {}",
            ih.name_interned, ih.expected_entries, ih.actual_entries
        );
    }
}

#[tokio::test]
async fn s9b_current_marker_skips_repair() {
    // When the marker is already 2, create() MUST NOT call repair()
    // (no observable state change beyond the cheap version read).
    let (t1, data, info) = fresh_table_with_index().await;

    // Confirm marker is already 2.
    assert_eq!(load_legacy_index_version(&info).await.unwrap(), 2);

    // Verify is healthy before reopen.
    assert!(t1.verify().await.unwrap().is_healthy());
    drop(t1);

    // Reopen — marker stays 2, table stays healthy.
    let t2 = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    assert_eq!(
        load_legacy_index_version(&info).await.unwrap(),
        2,
        "marker must stay 2 when already current"
    );
    assert!(
        t2.verify().await.unwrap().is_healthy(),
        "table must remain healthy on reopen with current marker"
    );
}

#[tokio::test]
async fn s9b_no_indexes_still_stamps_marker() {
    // A table with NO legacy indexes still gets the version marker stamped
    // so subsequent opens skip the version check entirely.
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // First open — no indexes, marker absent (version 0 → needs rebuild path).
    let _t1 = TableManager::create("empty".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    // Marker must be stamped even though there are no legacy indexes to rebuild.
    let v = load_legacy_index_version(&info).await.unwrap();
    assert_eq!(
        v, 2,
        "version marker must be written even for tables with no legacy indexes"
    );
}
