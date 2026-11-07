//! Identity migration on open (P34-12 S2) — rewritten from the S9b `lfv`
//! rebuild-on-open tests for the NEW marker semantics (`_m.idx.idv`).
//!
//! Proves that when a table is reopened whose base index family is not
//! identity-current (`need = !(idv == 2 && lfv absent)`), `TableManager::
//! create` runs the blocking base migration (remove markers → in-place
//! both-namespace sweep and rebuild of every definition → stamp idv = 2),
//! leaving every index consistent. The S9b helpers (`lfv` read/write) are kept only
//! to simulate OLD-binary data; new binaries never write `lfv`.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::index::index_record_key::IndexRecordKey;
use crate::index2::persistence::{
    load_index_identity_version, load_legacy_index_version, INDEX_IDENTITY_VERSION,
};
use crate::table::tests::behavior::lifecycle::pipeline::p34_12_identity_migration_tests::{
    count_prefix, legacy_v3_key,
};
use crate::table::tests::behavior::schema::test_helpers::query_value_to_inner_tracked;
use crate::table::TableManager;

/// Build shared in-memory stores, open a fresh table on them, create a
/// regular and a unique index, seed a few records, and return
/// (table, data_store, info_store).
/// After this call the table is stamped idv == 2 (by the create() migration
/// trigger on its first open).
async fn fresh_table_with_index() -> (TableManager, Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let t = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    t.create_index("by_x", &["x"]).await.unwrap();
    t.create_unique_index("uq_x", &["x"]).await.unwrap();

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

    // Baseline: healthy and already identity-current by the first create().
    let report = t1.verify().await.unwrap();
    assert!(
        report.is_healthy(),
        "fresh table must be healthy, got {report:?}"
    );
    let v = load_index_identity_version(&info).await.unwrap();
    assert_eq!(
        v,
        Some(INDEX_IDENTITY_VERSION),
        "first open must stamp the identity version marker"
    );
    // New binaries never write lfv.
    assert_eq!(
        load_legacy_index_version(&info).await.unwrap(),
        0,
        "lfv must stay absent after a new-binary open"
    );

    // -----------------------------------------------------------------------
    // 2. Simulate an OLD binary having touched the table: it rebuilds in v3
    //    and stamps lfv (the S9b semantics). The key mirrors the private
    //    `meta_key_legacy_index_version()` from shamir_index::persistence —
    //    duplicated here intentionally (with this comment) rather than
    //    making it more public, because the test is the only caller.
    // -----------------------------------------------------------------------
    let marker_key = RecordId::system("_m.idx.lfv").to_bytes();
    info.set(marker_key.into(), Bytes::from(3u32.to_le_bytes().to_vec()))
        .await
        .unwrap();

    assert_eq!(
        load_legacy_index_version(&info).await.unwrap(),
        3,
        "marker must be 3 after the old-binary simulation"
    );

    // -----------------------------------------------------------------------
    // 3. Plant the literal v3 postings an old binary's rebuild leaves behind,
    //    one per family (regular namespace 0x00, unique namespace 0x01), so
    //    the migration sweep has an observable effect in BOTH namespaces.
    // -----------------------------------------------------------------------
    let regular_defs: Vec<_> = t1.index_manager_ref().iter_indexes().collect();
    let def = regular_defs.first().expect("by_x index must exist");
    let unique_defs: Vec<_> = t1.index_manager_ref().iter_unique_indexes().collect();
    let unique_def = unique_defs.first().expect("uq_x index must exist");

    let fake_rid = RecordId::new();
    let mut legacy_regular = legacy_v3_key(false, def.name_interned);
    legacy_regular.extend_from_slice(fake_rid.as_bytes());
    info.set(Bytes::from(legacy_regular).into(), Bytes::new())
        .await
        .unwrap();
    info.set(
        Bytes::from(legacy_v3_key(true, unique_def.name_interned)).into(),
        Bytes::copy_from_slice(fake_rid.as_bytes()),
    )
    .await
    .unwrap();
    let legacy_regular_prefix =
        IndexRecordKey::new(false, def.name_interned).to_legacy_prefix_bytes();
    let legacy_unique_prefix =
        IndexRecordKey::new(true, unique_def.name_interned).to_legacy_prefix_bytes();
    assert_eq!(
        count_prefix(&info, legacy_regular_prefix.clone()).await,
        1,
        "legacy regular posting must be planted"
    );
    assert_eq!(
        count_prefix(&info, legacy_unique_prefix.clone()).await,
        1,
        "legacy unique posting must be planted"
    );

    // A v2 orphan under the namespace `verify()` counts, so the table is
    // observably unhealthy before the migration.
    let identity_key = crate::index::index_keys::build_index_key(
        false,
        def.name_interned,
        &[InnerValue::Int(9999)],
    )
    .unwrap()
    .to_bytes();
    let mut orphan_key = identity_key.to_vec();
    orphan_key.extend_from_slice(fake_rid.as_bytes());
    info.set(Bytes::from(orphan_key).into(), Bytes::new())
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
    // 4. Reopen on the SAME stores — the identity migration fires:
    //      identity_migration_needed() → true (lfv present)
    //      both namespaces of every definition are swept in place → the stale
    //        v3 postings and the v2 orphan are removed and v2 postings are
    //        rebuilt from the data store
    //      idv = 2 is stamped
    // -----------------------------------------------------------------------
    let t2 = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    // -----------------------------------------------------------------------
    // 5. Key assertions — non-vacuous because lfv was 3 before reopen.
    // -----------------------------------------------------------------------
    assert_eq!(
        load_index_identity_version(&info).await.unwrap(),
        Some(INDEX_IDENTITY_VERSION),
        "reopen must stamp idv = 2"
    );
    assert!(
        !shamir_index::persistence::legacy_index_version_is_present(&info)
            .await
            .unwrap(),
        "lfv must be absent after the migration"
    );

    let after = t2.verify().await.unwrap();
    assert!(
        after.is_healthy(),
        "migration on reopen must remove the stale v3 orphan and leave the table healthy: {after:?}"
    );
    assert_eq!(
        count_prefix(&info, legacy_regular_prefix).await,
        0,
        "migration must sweep legacy regular postings"
    );
    assert_eq!(
        count_prefix(&info, legacy_unique_prefix).await,
        0,
        "migration must sweep legacy unique postings"
    );

    // All 5 records must still be present and indexed.
    assert_eq!(after.records_in_data, 5, "record count must be preserved");
    assert_eq!(after.counter_value, 5, "counter must be recounted to 5");
    assert_eq!(
        after.unique_indexes.len(),
        1,
        "unique definition must survive"
    );
    for ih in after.regular_indexes.iter().chain(&after.unique_indexes) {
        assert_eq!(
            ih.expected_entries, ih.actual_entries,
            "index {}: expected {} actual {}",
            ih.name_interned, ih.expected_entries, ih.actual_entries
        );
    }

    // The lookups must answer through the rebuilt v2 postings.
    let interner = t2.interner().get().await.unwrap();
    for i in 0..5u32 {
        let mut m = new_map();
        m.insert("x".to_string(), QueryValue::Int(i as i64));
        let (iv, _new_keys) = query_value_to_inner_tracked(&QueryValue::Map(m), interner).unwrap();
        let InnerValue::Map(fields) = &iv else {
            panic!("record must decode to a map");
        };
        let x = fields.values().next().expect("x field").clone();
        let found = t2
            .lookup_by_index("by_x", std::slice::from_ref(&x))
            .await
            .unwrap();
        assert!(
            !found.is_empty(),
            "record with x={i} must be found after the identity migration"
        );
    }
}

#[tokio::test]
async fn s9b_current_marker_skips_repair() {
    // When the table is already identity-current (idv == 2 AND lfv absent),
    // create() MUST NOT run the migration (no observable state change
    // beyond the cheap marker reads).
    let (t1, data, info) = fresh_table_with_index().await;

    // Confirm the markers.
    assert_eq!(
        load_index_identity_version(&info).await.unwrap(),
        Some(INDEX_IDENTITY_VERSION)
    );

    // Verify is healthy before reopen.
    assert!(t1.verify().await.unwrap().is_healthy());
    drop(t1);

    // Reopen — markers stay, table stays healthy.
    let t2 = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    assert_eq!(
        load_index_identity_version(&info).await.unwrap(),
        Some(INDEX_IDENTITY_VERSION),
        "idv must stay current when already current"
    );
    assert!(
        t2.verify().await.unwrap().is_healthy(),
        "table must remain healthy on reopen with current markers"
    );
}

#[tokio::test]
async fn s9b_no_indexes_still_stamps_marker() {
    // A table with NO base indexes still gets the idv marker stamped
    // (steps 1 + 3 without the repair) so subsequent opens skip the
    // migration entirely.
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // First open — no indexes, markers absent.
    let _t1 = TableManager::create("empty".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    // Marker must be stamped even though there is nothing to rebuild.
    let v = load_index_identity_version(&info).await.unwrap();
    assert_eq!(
        v,
        Some(INDEX_IDENTITY_VERSION),
        "idv marker must be written even for tables with no base indexes"
    );
}
