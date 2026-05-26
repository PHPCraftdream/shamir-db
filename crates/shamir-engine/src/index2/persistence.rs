//! Persistence for index2 registry — save/load `IndexDescriptor`s
//! via `MetaEnvelope` to `__meta__/indexes`.

use crate::index2::descriptor::IndexDescriptor;
use crate::meta::MetaEnvelope;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::types::Store;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndexes {
    pub next_id: u32,
    pub descriptors: Vec<IndexDescriptor>,
}

pub async fn save_index2_metadata(
    registry: &crate::index2::IndexRegistry,
    info_store: &Arc<dyn Store>,
) -> Result<(), shamir_storage::error::DbError> {
    let p = PersistedIndexes {
        next_id: registry.peek_next_id(),
        descriptors: registry.all_descriptors().await,
    };
    let envelope = MetaEnvelope::new(p);
    let bytes = envelope
        .encode()
        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
    let key = crate::meta::MetaKey::Indexes.as_record_id();
    info_store
        .set(key.to_bytes(), Bytes::from(bytes))
        .await
        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
    Ok(())
}

pub async fn load_index2_metadata(
    info_store: &Arc<dyn Store>,
) -> Result<Option<PersistedIndexes>, shamir_storage::error::DbError> {
    let key = crate::meta::MetaKey::Indexes.as_record_id();
    match info_store.get(key.to_bytes()).await {
        Ok(bytes) => {
            let p: PersistedIndexes = MetaEnvelope::open(&bytes)
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            Ok(Some(p))
        }
        Err(shamir_storage::error::DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index2::kind::*;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use smallvec::SmallVec;

    #[tokio::test]
    async fn round_trip_save_load() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let registry = crate::index2::IndexRegistry::new();

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
        let _registry = crate::index2::IndexRegistry::new();

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
                expr: crate::index2::expr::IndexExpr::Lower(Box::new(
                    crate::index2::expr::IndexExpr::Field(vec![200]),
                )),
            })),
        );

        // Save manually constructed PersistedIndexes.
        let p = PersistedIndexes {
            next_id: 3,
            descriptors: vec![d1, d2],
        };
        let envelope = MetaEnvelope::new(p);
        let bytes = envelope.encode().unwrap();
        let key = crate::meta::MetaKey::Indexes.as_record_id();
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
}
