use crate::descriptor::IndexDescriptor;
use crate::kind::{FunctionalConfig, IndexKind, TokenizerKind};
use crate::persistence::{
    add_to_dropping_index2, legacy_indexes_need_rebuild, load_dropping_index2,
    load_index2_metadata, load_legacy_index_version, save_index2_metadata,
    save_legacy_index_version, PersistedIndexes, LEGACY_INDEX_FORMAT_VERSION,
};
use crate::MetaEnvelope;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use smallvec::SmallVec;
use std::sync::Arc;

// The meta key tag "_m.idx" is byte-identical to MetaKey::Indexes.tag() in the engine.
fn meta_key_indexes() -> RecordId {
    RecordId::system("_m.idx")
}

#[tokio::test]
async fn round_trip_save_load() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let registry = crate::IndexRegistry::new();

    // Allocate IDs to advance counter.
    let _ = registry.allocate_id();
    let _ = registry.allocate_id();

    save_index2_metadata(&registry, &store).await.unwrap();
    let loaded = load_index2_metadata(&store).await.unwrap().unwrap();
    assert_eq!(loaded.next_id, 3);
    assert!(loaded.descriptors.is_empty());
}

#[tokio::test]
async fn round_trip_with_descriptors() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let _registry = crate::IndexRegistry::new();

    // Simulate: 2 descriptors persisted (via save, not through registry —
    // just testing save/load serialization).
    let d1 = IndexDescriptor::new(
        1,
        "fts_body",
        100,
        SmallVec::new(),
        IndexKind::Fts {
            tokenizer: TokenizerKind::Whitespace,
            language: None,
        },
    );
    let d2 = IndexDescriptor::new(
        2,
        "lower_email",
        200,
        SmallVec::new(),
        IndexKind::Functional(Box::new(FunctionalConfig {
            expr: crate::expr::IndexExpr::Lower(Box::new(crate::expr::IndexExpr::Field(vec![200]))),
        })),
    );

    // Save manually constructed PersistedIndexes.
    let p = PersistedIndexes {
        next_id: 3,
        descriptors: vec![d1, d2],
    };
    let envelope = MetaEnvelope::new(p);
    let bytes = envelope.encode().unwrap();
    let key = meta_key_indexes();
    store
        .set(key.to_bytes().into(), Bytes::from(bytes))
        .await
        .unwrap();

    let loaded = load_index2_metadata(&store).await.unwrap().unwrap();
    assert_eq!(loaded.next_id, 3);
    assert_eq!(loaded.descriptors.len(), 2);
    assert_eq!(loaded.descriptors[0].name, "fts_body");
    assert_eq!(loaded.descriptors[1].name, "lower_email");
}

#[tokio::test]
async fn load_missing_returns_none() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let loaded = load_index2_metadata(&store).await.unwrap();
    assert!(loaded.is_none());
}

// ============================================================================
// S9 — legacy index format version
// ============================================================================

#[tokio::test]
async fn legacy_version_missing_returns_zero() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let v = load_legacy_index_version(&store).await.unwrap();
    assert_eq!(v, 0, "missing version marker must return 0");
}

#[tokio::test]
async fn legacy_version_save_load_roundtrip() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    save_legacy_index_version(&store).await.unwrap();
    let v = load_legacy_index_version(&store).await.unwrap();
    assert_eq!(v, LEGACY_INDEX_FORMAT_VERSION);
}

#[tokio::test]
async fn legacy_needs_rebuild_when_missing() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    assert!(
        legacy_indexes_need_rebuild(&store).await.unwrap(),
        "pre-S9 data (no version marker) must trigger rebuild"
    );
}

#[tokio::test]
async fn legacy_no_rebuild_when_current() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    save_legacy_index_version(&store).await.unwrap();
    assert!(
        !legacy_indexes_need_rebuild(&store).await.unwrap(),
        "current version must NOT trigger rebuild"
    );
}

#[tokio::test]
async fn legacy_rebuild_when_old_version() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // Simulate an old version (1) in the store.
    let key = RecordId::system("_m.idx.lfv");
    let old_ver: u32 = 1;
    store
        .set(
            key.to_bytes().into(),
            Bytes::from(old_ver.to_le_bytes().to_vec()),
        )
        .await
        .unwrap();
    assert!(
        legacy_indexes_need_rebuild(&store).await.unwrap(),
        "old version must trigger rebuild"
    );
}

// ============================================================================
// #1051 — index2 DROP tombstone backward compatibility
// ============================================================================

/// #1051: a pre-#1051 index2 DROP tombstone (bare `Vec<u32>`, no name/op_id
/// fields) must still load correctly, with every entry given an empty name
/// and `op_id: None`.
#[tokio::test]
async fn p1051_old_format_index2_drop_tombstone_decodes_with_empty_name_and_none_op_id() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let old_format: Vec<u32> = vec![10, 20];
    let key = RecordId::system("_m.idx.drop").to_bytes();
    let bytes = bincode::serialize(&old_format).unwrap();
    store.set(key.into(), Bytes::from(bytes)).await.unwrap();

    let loaded = load_dropping_index2(&store).await.unwrap();
    assert_eq!(
        loaded,
        vec![(10, String::new(), None), (20, String::new(), None)],
        "old-format Vec<u32> tombstone must decode with empty name and op_id: None"
    );
}

/// #1051: a NEW-format index2 DROP tombstone round-trips its name and op_id
/// through `add_to_dropping_index2` / `load_dropping_index2`.
#[tokio::test]
async fn p1051_new_format_index2_drop_tombstone_round_trips_name_and_op_id() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let op_id = RecordId::new();
    add_to_dropping_index2(5, "lower_name".to_string(), Some(op_id.to_string()), &store)
        .await
        .unwrap();
    add_to_dropping_index2(6, "upper_name".to_string(), None, &store)
        .await
        .unwrap();

    let loaded = load_dropping_index2(&store).await.unwrap();
    assert_eq!(
        loaded,
        vec![
            (5, "lower_name".to_string(), Some(op_id.to_string())),
            (6, "upper_name".to_string(), None),
        ],
        "new-format tombstone must round-trip name and op_id exactly"
    );
}
