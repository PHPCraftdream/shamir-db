use crate::meta::MetaKey;
use crate::table::interner_manager::InternerManager;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::codecs::basic::bincode;
use shamir_types::core::interner::{InternerKey, UserKey};
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

async fn create_manager() -> InternerManager {
    InternerManager::new(Arc::new(InMemoryStore::new()))
}

#[tokio::test]
async fn test_interner_lazy_loading() {
    let manager = create_manager().await;

    let interner = manager.get().await.unwrap();
    assert_eq!(interner.len(), 0);

    let interner2 = manager.get().await.unwrap();
    assert!(std::ptr::eq(interner, interner2));
}

#[tokio::test]
async fn test_interner_save_new_keys() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager1 = InternerManager::new(Arc::clone(&store));
    let manager2 = InternerManager::new(store);

    let interner1 = manager1.get().await.unwrap();
    let result1 = interner1.touch_ind("name").unwrap();
    let result1_id = result1.key().id();
    let new_keys = vec![(result1.into_key(), UserKey::from_str("name"))];
    manager1.save_new_keys(&new_keys).await.unwrap();

    let interner2 = manager2.get().await.unwrap();
    let result2 = interner2.touch_ind("name").unwrap();

    assert_eq!(result1_id, result2.key().id());
}

#[tokio::test]
async fn test_interner_persistence() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager1 = InternerManager::new(Arc::clone(&store));

    let interner1 = manager1.get().await.unwrap();
    let name_key = interner1.touch_ind("name").unwrap();
    let age_key = interner1.touch_ind("age").unwrap();
    let new_keys = vec![
        (name_key.key().clone(), UserKey::from_str("name")),
        (age_key.key().clone(), UserKey::from_str("age")),
    ];
    manager1.save_new_keys(&new_keys).await.unwrap();

    let manager2 = InternerManager::new(store);
    let interner2 = manager2.get().await.unwrap();

    assert_eq!(interner2.len(), 2);
    assert_eq!(
        interner2.touch_ind("name").unwrap().key().id(),
        name_key.key().id()
    );
    assert_eq!(
        interner2.touch_ind("age").unwrap().key().id(),
        age_key.key().id()
    );
}

#[tokio::test]
async fn test_interner_empty_save() {
    let manager = create_manager().await;

    let result = manager.save_new_keys(&[]).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_interner_multiple_saves() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager = InternerManager::new(store);

    let interner = manager.get().await.unwrap();

    let key1 = interner.touch_ind("key1").unwrap();
    manager
        .save_new_keys(&[(key1.into_key(), UserKey::from_str("key1"))])
        .await
        .unwrap();

    let key2 = interner.touch_ind("key2").unwrap();
    manager
        .save_new_keys(&[(key2.into_key(), UserKey::from_str("key2"))])
        .await
        .unwrap();

    assert_eq!(interner.len(), 2);
}

/// Incremental persist writes one chunk per call, reload reconstructs
/// the identical dictionary (every id → name mapping preserved).
#[tokio::test]
async fn test_interner_incremental_persist_then_reload() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager = InternerManager::new(Arc::clone(&store));

    let interner = manager.get().await.unwrap();
    let mut expected: Vec<(u64, String)> = Vec::new();
    for i in 0..50usize {
        let name = format!("field_{i}");
        let t = interner.touch_ind(&name).unwrap();
        expected.push((t.key().id(), name));
        // Persist after every touch — exercises the per-op call path.
        manager.persist().await.unwrap();
    }

    // Reload into a fresh manager wired to the same store.
    let manager2 = InternerManager::new(store);
    let interner2 = manager2.get().await.unwrap();
    assert_eq!(interner2.len(), expected.len());
    for (id, name) in &expected {
        let key = interner2.make_key(*id);
        let got = interner2.get_str(&key).expect("id must resolve to name");
        assert_eq!(&*got, name.as_str(), "id {} mismapped", id);
        // Reverse: name → id round-trips.
        let id2 = interner2.get_ind(name).expect("name must intern");
        assert_eq!(id2.id(), *id, "name {} mismapped", name);
    }
}

/// OLD-format single-blob written under MetaKey::Internals must load
/// correctly under new code (backward-compat). After load, new
/// persists chain on top as delta chunks.
#[tokio::test]
async fn test_interner_legacy_blob_loads_under_new_code() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Hand-write a legacy blob: a Vec<(InternerKey, UserKey)> serialized
    // with bincode under MetaKey::Internals — mirrors what the old
    // persist() used to write.
    let legacy_entries = vec![
        (InternerKey::new(1), UserKey::from_str("alpha")),
        (InternerKey::new(2), UserKey::from_str("beta")),
        (InternerKey::new(3), UserKey::from_str("gamma")),
    ];
    let bytes = bincode::to_bytes(&legacy_entries).unwrap();
    store
        .set(MetaKey::Internals.as_record_id().to_bytes().into(), bytes)
        .await
        .unwrap();

    // New code reads the legacy blob.
    let manager = InternerManager::new(Arc::clone(&store));
    let interner = manager.get().await.unwrap();
    assert_eq!(interner.len(), 3);
    assert_eq!(&*interner.get_str(&InternerKey::new(1)).unwrap(), "alpha");
    assert_eq!(&*interner.get_str(&InternerKey::new(2)).unwrap(), "beta");
    assert_eq!(&*interner.get_str(&InternerKey::new(3)).unwrap(), "gamma");

    // Append a fresh entry — should land as a delta chunk, not as a
    // rewrite of the legacy blob.
    let touch = interner.touch_ind("delta").unwrap();
    assert_eq!(touch.key().id(), 4);
    manager.persist().await.unwrap();

    // Reload into yet another fresh manager — legacy blob + delta
    // chunk must combine into the full dictionary.
    let manager2 = InternerManager::new(store);
    let interner2 = manager2.get().await.unwrap();
    assert_eq!(interner2.len(), 4);
    for (id, name) in [(1u64, "alpha"), (2, "beta"), (3, "gamma"), (4, "delta")] {
        assert_eq!(&*interner2.get_str(&InternerKey::new(id)).unwrap(), name);
    }
}

/// Concurrent touch + persist must converge to a consistent state:
/// every interned id resolves to its name after reload.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_interner_concurrent_touch_and_persist() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager = InternerManager::new(Arc::clone(&store));

    // Force initial load so all clones share the same interner.
    let _ = manager.get().await.unwrap();

    let mut handles = Vec::new();
    for t in 0..4usize {
        let mgr = manager.clone();
        handles.push(tokio::spawn(async move {
            let mut names = Vec::new();
            for i in 0..100usize {
                let name = format!("t{}_f{}", t, i);
                let interner = mgr.get().await.unwrap();
                let touch = interner.touch_ind(&name).unwrap();
                names.push((touch.key().id(), name));
                mgr.persist().await.unwrap();
            }
            names
        }));
    }

    let mut all_pairs: Vec<(u64, String)> = Vec::new();
    for h in handles {
        all_pairs.extend(h.await.unwrap());
    }

    // Reload and verify every name we touched still resolves to its id.
    let manager2 = InternerManager::new(store);
    let interner2 = manager2.get().await.unwrap();
    assert_eq!(interner2.len(), 400);
    for (id, name) in &all_pairs {
        let key = interner2.make_key(*id);
        let got = interner2
            .get_str(&key)
            .unwrap_or_else(|| panic!("id {} (name {}) not found after reload", id, name));
        assert_eq!(&*got, name.as_str());
    }
}

/// Audit §2.6 regression: a corrupt delta chunk must be a FATAL open error,
/// not a silent skip-and-continue. Before the fix, a corrupt chunk was
/// logged and skipped — continuing with a truncated dictionary means minting
/// new ids over already-occupied ones, silently corrupting every old record
/// referencing those ids. Now `get()` must return `Err`.
#[tokio::test]
async fn corrupt_delta_chunk_is_fatal() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Write ONE valid chunk (one entry) so the dictionary has state.
    let valid_chunk = vec![(InternerKey::new(1), UserKey::from_str("alpha"))];
    let chunk_key = RecordId::system("i.d000000000").to_bytes();
    store
        .set(
            chunk_key.clone().into(),
            bincode::to_bytes(&valid_chunk).unwrap(),
        )
        .await
        .unwrap();

    // Write a SECOND chunk that is garbage (not valid bincode).
    let corrupt_key = RecordId::system("i.d000000001").to_bytes();
    store
        .set(
            corrupt_key.into(),
            Bytes::from_static(b"NOT_VALID_BINCODE_GARBAGE"),
        )
        .await
        .unwrap();

    // Loading must FAIL — the corrupt chunk cannot be silently skipped.
    let manager = InternerManager::new(store);
    let result = manager.get().await;
    assert!(
        result.is_err(),
        "corrupt interner chunk must be fatal, not silently skipped (audit §2.6)"
    );
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("§2.6") || err.contains("corrupt") || err.contains("deserialize"),
        "error must indicate corruption is fatal, got: {err}"
    );
}

/// Audit §2.6 regression: a corrupt LEGACY blob (MetaKey::Internals) must
/// be a FATAL open error, not silently become "empty dictionary". Before the
/// fix, `unwrap_or_else(|e| { log::error!(...); Vec::new() })` turned a
/// corrupt legacy blob into an empty seed — then chunk scan found nothing,
/// and the interner started from zero, re-minting ids over old records.
#[tokio::test]
async fn corrupt_legacy_blob_is_fatal() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Write a corrupt legacy blob under MetaKey::Internals.
    store
        .set(
            MetaKey::Internals.as_record_id().to_bytes().into(),
            Bytes::from_static(b"CORRUPT_LEGACY_BLOB_GARBAGE"),
        )
        .await
        .unwrap();

    // Loading must FAIL.
    let manager = InternerManager::new(store);
    let result = manager.get().await;
    assert!(
        result.is_err(),
        "corrupt legacy interner blob must be fatal, not silently emptied (audit §2.6)"
    );
}
