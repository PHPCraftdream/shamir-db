//! Persistence for index2 registry — save/load `IndexDescriptor`s
//! via `MetaEnvelope` to `__meta__/indexes`.

use crate::descriptor::IndexDescriptor;
use crate::MetaEnvelope;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

// The meta key tag "_m.idx" is byte-identical to MetaKey::Indexes.tag() in the engine.
fn meta_key_indexes() -> RecordId {
    RecordId::system("_m.idx")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndexes {
    pub next_id: u32,
    pub descriptors: Vec<IndexDescriptor>,
}

pub async fn save_index2_metadata(
    registry: &crate::IndexRegistry,
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
    let key = meta_key_indexes();
    info_store
        .set(key.to_bytes(), Bytes::from(bytes))
        .await
        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
    Ok(())
}

pub async fn load_index2_metadata(
    info_store: &Arc<dyn Store>,
) -> Result<Option<PersistedIndexes>, shamir_storage::error::DbError> {
    let key = meta_key_indexes();
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
