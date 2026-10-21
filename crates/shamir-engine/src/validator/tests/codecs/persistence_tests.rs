use crate::validator::persistence::{load_validators_metadata, save_validators_metadata};
use crate::validator::{ValidatorBinding, WriteOp};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use smallvec::smallvec;
use std::sync::Arc;

#[tokio::test]
async fn round_trip_save_load() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let bindings = vec![
        ValidatorBinding {
            validator_id: RecordId::system("val_a"),
            ops: smallvec![WriteOp::Insert, WriteOp::Update],
            priority: 1000,
        },
        ValidatorBinding {
            validator_id: RecordId::system("val_b"),
            ops: smallvec![WriteOp::Delete],
            priority: 5000,
        },
    ];

    save_validators_metadata(&bindings, &store).await.unwrap();
    let loaded = load_validators_metadata(&store).await.unwrap().unwrap();

    assert_eq!(loaded.bindings.len(), 2);
    assert_eq!(loaded.bindings[0].validator_id, RecordId::system("val_a"));
    assert_eq!(
        loaded.bindings[0].ops.as_slice(),
        &[WriteOp::Insert, WriteOp::Update]
    );
    assert_eq!(loaded.bindings[0].priority, 1000);
    assert_eq!(loaded.bindings[1].validator_id, RecordId::system("val_b"));
    assert_eq!(loaded.bindings[1].ops.as_slice(), &[WriteOp::Delete]);
    assert_eq!(loaded.bindings[1].priority, 5000);
}

#[tokio::test]
async fn round_trip_empty() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    save_validators_metadata(&[], &store).await.unwrap();
    let loaded = load_validators_metadata(&store).await.unwrap().unwrap();
    assert!(loaded.bindings.is_empty());
}

#[tokio::test]
async fn load_missing_returns_none() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let loaded = load_validators_metadata(&store).await.unwrap();
    assert!(loaded.is_none());
}
