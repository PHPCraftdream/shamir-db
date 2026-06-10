use crate::descriptor::IndexDescriptor;
use crate::kind::{FunctionalConfig, IndexKind, TokenizerKind};
use crate::persistence::{load_index2_metadata, save_index2_metadata, PersistedIndexes};
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
    store.set(key.to_bytes(), Bytes::from(bytes)).await.unwrap();

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
