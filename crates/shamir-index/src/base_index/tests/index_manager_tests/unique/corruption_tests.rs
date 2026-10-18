use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_keys::build_index_key;
use crate::base_index::index_manager::IndexManager;
use crate::base_index::tests::index_manager_tests::helpers::{create_manager, create_test_value};
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

// ============================================================================
// validate_unique_for_create_with_defs — unit tests
// ============================================================================

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_empty_defs_early_exit() {
    let (_, _, manager) = create_manager();

    // Even if a unique index exists, empty defs slice → Ok immediately.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    // Seed a record so a real check would fail.
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &value)
        .await
        .unwrap();

    // Empty defs — should return Ok regardless of index state.
    let result = manager
        .validate_unique_for_create_with_defs(&value, &[])
        .await;
    assert!(
        result.is_ok(),
        "empty defs must return Ok, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_single_def_accept() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_unique_index(index_def.clone())
        .await
        .unwrap();

    let existing = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &existing)
        .await
        .unwrap();

    // Different value — should pass.
    let new_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let defs = vec![index_def];
    let result = manager
        .validate_unique_for_create_with_defs(&new_value, &defs)
        .await;
    assert!(
        result.is_ok(),
        "non-duplicate should pass, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_single_def_reject() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_unique_index(index_def.clone())
        .await
        .unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &value)
        .await
        .unwrap();

    // Same value — must reject with DuplicateKey.
    let defs = vec![index_def];
    let result = manager
        .validate_unique_for_create_with_defs(&value, &defs)
        .await;
    assert!(result.is_err(), "duplicate should be rejected");
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::DuplicateKey(_)
        ),
        "expected DuplicateKey error"
    );
}

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_multi_def_trip_on_second() {
    let (_, _, manager) = create_manager();

    let def1 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let def2 = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![2])]);
    manager.create_unique_index(def1.clone()).await.unwrap();
    manager.create_unique_index(def2.clone()).await.unwrap();

    // Seed "Alice" in index 1001 and "X" in index 1002.
    let existing = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Str("X".to_string())),
    ]);
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &existing)
        .await
        .unwrap();

    // New value has unique field-1 but duplicates field-2 — should trip on def2.
    let new_value = create_test_value(&[
        (1, InnerValue::Str("Bob".to_string())),
        (2, InnerValue::Str("X".to_string())),
    ]);
    let defs = vec![def1, def2];
    let result = manager
        .validate_unique_for_create_with_defs(&new_value, &defs)
        .await;
    assert!(result.is_err(), "should trip on the second def");
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::DuplicateKey(_)
        ),
        "expected DuplicateKey error"
    );
}

// ============================================================================
// plan_* tests (Stage 1.1.E) — unique index collision
// ============================================================================

#[tokio::test]
async fn unique_index_collision_in_plan_phase() {
    let data = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let mgr = IndexManager::new(Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    let idx = IndexDefinition::new(5001, vec![IndexInfoItem::new(vec![1])]);
    mgr.create_unique_index(idx).await.unwrap();

    let rid1 = RecordId::new();
    let val = create_test_value(&[(1, InnerValue::Str("dup".to_string()))]);

    // Insert the first record.
    mgr.validate_unique_for_create(&val).await.unwrap();
    mgr.on_record_created_unique(&rid1, &val).await.unwrap();

    // Second record with same value — should fail with DuplicateKey.
    let result = mgr.validate_unique_for_create(&val).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        shamir_storage::error::DbError::DuplicateKey(msg) => {
            assert!(msg.contains("5001"), "error should mention index name");
        }
        other => panic!("expected DuplicateKey, got {:?}", other),
    }
}

// ============================================================================
// P0-4 (#960) — corrupt unique posting must fail CLOSED (Err), not Ok(None)
// ============================================================================
//
// A unique-index posting value MUST be exactly a 16-byte RecordId. Any other
// length is genuine corruption. The fix changes `check_unique_key`'s prior
// behavior (which logged a warning and returned `Ok(None)` — the SAME signal
// callers use for "key is free") to surface a typed `DbError::Codec`, so a
// write that would have relied on this check aborts instead of silently
// committing a duplicate on top of corrupted storage. These tests prove:
//   * every malformed length (0/15/17/64) surfaces an error via the lookup
//     path (`lookup_by_unique_index` → `check_unique_constraint` →
//     `check_unique_key`);
//   * the genuine not-found path (key never written) is UNCHANGED and still
//     returns `Ok(None)`;
//   * a well-formed 16-byte posting still decodes to `Ok(Some(RecordId))`;
//   * `validate_unique_for_create` / `validate_unique_for_update` against a
//     corrupted posting ABORT (Err), not silently succeed.

/// Write `corrupt_value` into the info_store at the exact 25-byte unique-index
/// key that `check_unique_key` reads for `(name_interned, lookup_values)`.
/// Simulates a torn/corrupt posting left behind by a crash or a buggy writer.
async fn seed_corrupt_unique_posting(
    info_store: &Arc<dyn Store>,
    name_interned: u64,
    lookup_values: &[InnerValue],
    corrupt_value: Bytes,
) {
    let index_key = build_index_key(true, name_interned, lookup_values).to_bytes();
    info_store
        .set(index_key.into(), corrupt_value)
        .await
        .unwrap();
}

/// Shared harness: build a manager with one unique index on field 1 and return
/// its info_store handle (for seeding corruption) plus the manager.
async fn manager_with_unique_index() -> (Arc<dyn Store>, IndexManager) {
    let (_, info_store, manager) = create_manager();
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();
    (info_store, manager)
}

#[tokio::test]
async fn corrupt_unique_posting_length_0_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::new(),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "0-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn corrupt_unique_posting_length_15_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0xAB; 15]),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "15-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn corrupt_unique_posting_length_17_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0xCD; 17]),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "17-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn corrupt_unique_posting_length_64_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0xEF; 64]),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "64-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn genuine_not_found_unique_posting_returns_ok_none() {
    // Regression guard: the genuine "key is free" path (key NEVER written)
    // must remain Ok(None) — the corruption fix must NOT bleed into the
    // legitimate NotFound arm.
    let (_, manager) = manager_with_unique_index().await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("NeverWritten".to_string())])
        .await;
    assert!(
        result.is_ok(),
        "genuine NotFound must remain Ok, got {:?}",
        result
    );
    assert!(
        result.unwrap().is_none(),
        "a never-written key must resolve to None (free)"
    );
}

#[tokio::test]
async fn valid_16_byte_unique_posting_returns_ok_some() {
    // Positive control: a well-formed 16-byte RecordId posting decodes
    // correctly — the corruption fix did not break the happy path.
    let (info_store, manager) = manager_with_unique_index().await;
    let rid = RecordId::new();
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::copy_from_slice(rid.as_bytes()),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();
    assert_eq!(
        result,
        Some(rid),
        "16-byte posting must decode to its RecordId"
    );
}

#[tokio::test]
async fn corrupt_unique_posting_aborts_validate_create() {
    // A create against a corrupted unique posting must ABORT (Err), not
    // silently pass validation and commit a duplicate on top of corruption.
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0x99; 12]),
    )
    .await;

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let result = manager.validate_unique_for_create(&value).await;
    assert!(
        result.is_err(),
        "create validation against a corrupt posting must abort, got {:?}",
        result
    );
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error to propagate to the create path"
    );
}

#[tokio::test]
async fn corrupt_unique_posting_aborts_validate_update() {
    // An update whose NEW value collides with a corrupted posting must ABORT
    // (Err). The update path (`validate_unique_for_update`) reads the same
    // posting via `check_unique_key`, so corruption must propagate there too.
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0x77; 20]),
    )
    .await;

    let record_id = RecordId::new();
    // The updating record currently holds a DIFFERENT value ("Bob"), and wants
    // to move to the corrupted value ("Alice"). The check_unique_key read of
    // "Alice" must surface the corruption error.
    let old_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let new_value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let result = manager
        .validate_unique_for_update(&record_id, &old_value, &new_value)
        .await;
    assert!(
        result.is_err(),
        "update validation against a corrupt posting must abort, got {:?}",
        result
    );
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error to propagate to the update path"
    );
}

// ============================================================================
// P2 (#1023) — create_unique_index backfill must fail CLOSED on a malformed
// key / undecodable value, not silently skip the row.
// ============================================================================
//
// Before the fix, `create_unique_index`'s live `data_store.iter_stream` scan
// `continue`d past a record whose key was not exactly 16 bytes, or whose
// value bytes failed `InnerValue::from_bytes`. That row never got a unique
// posting written, but the backfill still reported success — a later insert
// with a colliding value could be wrongly accepted because the original
// occupant's posting was never recorded. Mirrors #960's fail-closed fix for
// a corrupt EXISTING posting (`DbError::Codec`, not a silent `Ok`/skip).

#[tokio::test]
async fn backfill_aborts_on_malformed_key_instead_of_skipping() {
    let (data_store, _info_store, manager) = create_manager();

    // One well-formed record...
    let good_id = RecordId::new();
    let good_value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    data_store
        .set(good_id.to_bytes().into(), good_value.to_bytes().unwrap())
        .await
        .unwrap();

    // ...and one record under a MALFORMED key (17 bytes, not the required
    // 16-byte RecordId). This simulates a foreign/corrupt entry landing in
    // the data store's keyspace.
    let malformed_key: Vec<u8> = vec![0xAB; 17];
    let malformed_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    data_store
        .set(malformed_key.into(), malformed_value.to_bytes().unwrap())
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let result = manager.create_unique_index(index_def).await;

    assert!(
        result.is_err(),
        "backfill over a malformed key must return Err, not silently skip the row \
         and report success; got {result:?}"
    );
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected a typed Codec error for the malformed key (mirrors #960's \
         corrupt-posting fail-closed policy)"
    );

    // The abort must leave NO live unique index behind — the backfill never
    // reached `create_unique_index_from_records`' registration step, so
    // there is no half-populated index masquerading as complete.
    assert!(
        !manager.unique_index_exists(1001),
        "an aborted backfill must not leave a live (necessarily incomplete) unique index"
    );
}

#[tokio::test]
async fn backfill_aborts_on_undecodable_value_instead_of_skipping() {
    let (data_store, _info_store, manager) = create_manager();

    // One well-formed record...
    let good_id = RecordId::new();
    let good_value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    data_store
        .set(good_id.to_bytes().into(), good_value.to_bytes().unwrap())
        .await
        .unwrap();

    // ...and one record with a well-formed 16-byte key but GARBAGE value
    // bytes that fail `InnerValue::from_bytes` (not valid MessagePack).
    // `0xC1` is msgpack's reserved/never-used marker byte — guaranteed to
    // fail ANY msgpack decode (unlike e.g. `0xFF`, a valid negative-fixint
    // that `rmp_serde`'s `deserialize_any` happily accepts as `Int(-1)`
    // without requiring the rest of the buffer to be consumed). Same
    // reserved-byte technique `sorted_index_manager_tests::covering_tests`
    // uses to seed a genuinely undecodable blob.
    let garbage_id = RecordId::new();
    data_store
        .set(
            garbage_id.to_bytes().into(),
            Bytes::from_static(&[0xC1u8; 8]),
        )
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![1])]);
    let result = manager.create_unique_index(index_def).await;

    assert!(
        result.is_err(),
        "backfill over an undecodable value must return Err, not silently skip \
         the row and report success; got {result:?}"
    );
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected a typed Codec error for the undecodable value"
    );
    assert!(
        !manager.unique_index_exists(1002),
        "an aborted backfill must not leave a live (necessarily incomplete) unique index"
    );
}
