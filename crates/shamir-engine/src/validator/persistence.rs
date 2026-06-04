//! Persistence for per-table validator bindings — save/load
//! `PersistedValidators` via `MetaEnvelope` to `__meta__/validators`.
//!
//! Mirrors `crate::index2::persistence` exactly.

use crate::meta::MetaEnvelope;
use crate::validator::PersistedValidators;
use bytes::Bytes;
use shamir_storage::error::DbError;
use shamir_storage::types::Store;
use std::sync::Arc;

/// Persist the given validator bindings into the table's info-twin
/// under `MetaKey::Validators`.
pub async fn save_validators_metadata(
    bindings: &[crate::validator::ValidatorBinding],
    info_store: &Arc<dyn Store>,
) -> Result<(), DbError> {
    let p = PersistedValidators {
        bindings: bindings.to_vec(),
    };
    let envelope = MetaEnvelope::new(p);
    let bytes = envelope
        .encode()
        .map_err(|e| DbError::Internal(e.to_string()))?;
    let key = crate::meta::MetaKey::Validators.as_record_id();
    info_store
        .set(key.to_bytes(), Bytes::from(bytes))
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?;
    Ok(())
}

/// Load the persisted validator bindings from the table's info-twin.
/// Returns `Ok(None)` when no bindings have been saved yet.
pub async fn load_validators_metadata(
    info_store: &Arc<dyn Store>,
) -> Result<Option<PersistedValidators>, DbError> {
    let key = crate::meta::MetaKey::Validators.as_record_id();
    match info_store.get(key.to_bytes()).await {
        Ok(bytes) => {
            let p: PersistedValidators =
                MetaEnvelope::open(&bytes).map_err(|e| DbError::Internal(e.to_string()))?;
            Ok(Some(p))
        }
        Err(DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::{ValidatorBinding, WriteOp};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::types::record_id::RecordId;
    use smallvec::smallvec;

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
}
